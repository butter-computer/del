//! Read-only snapshots of Del-owned process trees. Command arguments (which may
//! contain secrets) are deliberately never collected or displayed.
use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Process {
    pub pid: u32,
    pub name: String,
    pub ports: Vec<String>,
    pub background: bool,
    pub owner: Option<String>,
}
#[derive(Clone)]
pub struct Root {
    pub pid: u32,
    /// None for a shared provider server or a global shell: never guess ownership.
    pub owner: Option<String>,
}
#[derive(Clone, Default)]
pub struct Snapshot {
    pub rows: Vec<Process>,
    pub error: Option<String>,
    pub ready: bool,
}
fn parse(text: &str, roots: &[Root], app: u32) -> Vec<Process> {
    let processes: BTreeMap<u32, (u32, String, bool)> = text
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (pid, rest) = line.split_once(char::is_whitespace)?;
            let (parent, rest) = rest.trim_start().split_once(char::is_whitespace)?;
            let (state, name) = rest.trim_start().split_once(char::is_whitespace)?;
            let name = std::path::Path::new(name.trim())
                .file_name()?
                .to_string_lossy()
                .into_owned();
            Some((
                pid.parse().ok()?,
                (parent.parse().ok()?, name, state.starts_with('Z')),
            ))
        })
        .collect();
    let mut owned: HashSet<u32> = roots.iter().map(|r| r.pid).chain([app]).collect();
    loop {
        let before = owned.len();
        for (pid, (parent, _, _)) in &processes {
            if owned.contains(parent) {
                owned.insert(*pid);
            }
        }
        if before == owned.len() {
            break;
        }
    }
    // Codex keeps its REPL host and Node wrapper alive between tool calls.
    // Identify by executable names/parentage, without inspecting command arguments.
    let repl_hosts: HashSet<u32> = processes
        .iter()
        .filter(|(_, (_, name, _))| name == "node_repl")
        .flat_map(|(pid, (parent, _, _))| {
            let wrapper = processes
                .get(parent)
                .is_some_and(|(_, name, _)| name == "node");
            std::iter::once(*pid).chain(wrapper.then_some(*parent))
        })
        .collect();
    let owners: BTreeMap<u32, Option<String>> = processes
        .keys()
        .map(|pid| {
            let mut cursor = *pid;
            let mut visited = HashSet::new();
            let mut owner = None;
            while visited.insert(cursor) {
                let matches = roots.iter().filter(|r| r.pid == cursor).collect::<Vec<_>>();
                if !matches.is_empty() {
                    let first = matches[0].owner.clone();
                    if matches.iter().all(|r| r.owner == first) {
                        owner = first;
                    }
                    break;
                }
                let Some((parent, _, _)) = processes.get(&cursor) else {
                    break;
                };
                cursor = *parent;
            }
            (*pid, owner)
        })
        .collect();
    processes
        .into_iter()
        .filter(|(pid, (parent, name, exited))| {
            let command = name.trim_matches(['(', ')']);
            !exited
                && owned.contains(pid)
                && *pid != app
                && !matches!(command, "ps" | "lsof")
                // Ignore Del's Git inspection; commands beneath agents and shells remain visible.
                && !(*parent == app && command == "git")
        })
        .map(|(pid, (parent, name, _))| {
            let command = name.trim_matches(['(', ')']).trim_start_matches('-');
            let background = matches!(command, "claude" | "codex" | "opencode")
                || repl_hosts.contains(&pid)
                || roots.iter().any(|r| r.pid == pid)
                || (parent == app
                    && matches!(
                        command,
                        "zsh" | "bash" | "sh" | "fish" | "claude" | "codex" | "opencode"
                    ));
            Process {
                pid,
                name,
                ports: vec![],
                background,
                owner: owners.get(&pid).cloned().flatten(),
            }
        })
        .collect()
}
fn add_ports(rows: &mut [Process], text: &str) {
    let mut pid = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value.parse::<u32>().ok();
        } else if let Some(address) = line.strip_prefix('n')
            && let Some(row) = rows.iter_mut().find(|row| Some(row.pid) == pid)
            && let Some((_, port)) = address.rsplit_once(':')
            && port.parse::<u16>().is_ok()
            && !row.ports.iter().any(|p| p == port)
        {
            row.ports.push(port.into());
        }
    }
}
async fn output(program: &str, args: &[String]) -> Option<std::process::Output> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    tokio::time::timeout(Duration::from_secs(3), command.output())
        .await
        .ok()?
        .ok()
}
pub async fn read(roots: Vec<Root>) -> Snapshot {
    let Some(ps) = output("/bin/ps", &["-axo".into(), "pid=,ppid=,stat=,comm=".into()])
        .await
        .filter(|o| o.status.success())
    else {
        return Snapshot {
            error: Some("Could not read running processes".into()),
            ready: true,
            ..Default::default()
        };
    };
    let mut rows = parse(
        &String::from_utf8_lossy(&ps.stdout),
        &roots,
        std::process::id(),
    );
    if !rows.is_empty() {
        let pids = rows
            .iter()
            .map(|p| p.pid.to_string())
            .collect::<Vec<_>>()
            .join(",");
        if let Some(ports) = output(
            "/usr/sbin/lsof",
            &[
                "-nP".into(),
                "-a".into(),
                "-p".into(),
                pids,
                "-iTCP".into(),
                "-sTCP:LISTEN".into(),
                "-Fpn".into(),
            ],
        )
        .await
        {
            add_ports(&mut rows, &String::from_utf8_lossy(&ports.stdout));
        }
    }
    rows.sort_by(|a, b| {
        b.ports
            .len()
            .cmp(&a.ports.len())
            .then(a.name.cmp(&b.name))
            .then(a.pid.cmp(&b.pid))
    });
    Snapshot {
        rows,
        error: None,
        ready: true,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_provider_is_unassigned_and_nested_terminal_owns_its_commands() {
        let rows = parse(
            "10 1 S /app/del\n20 10 S codex\n21 20 S node\n30 10 S zsh\n31 30 S python\n32 31 S worker\n40 20 S sh\n41 40 S node",
            &[
                Root {
                    pid: 20,
                    owner: None,
                },
                Root {
                    pid: 30,
                    owner: Some("chat:a".into()),
                },
                Root {
                    pid: 40,
                    owner: Some("chat:b".into()),
                },
            ],
            10,
        );
        let owner = |pid| rows.iter().find(|r| r.pid == pid).unwrap().owner.as_deref();
        assert_eq!(owner(21), None);
        assert_eq!(owner(31), Some("chat:a"));
        assert_eq!(owner(32), Some("chat:a"));
        assert_eq!(owner(41), Some("chat:b"));
    }
    #[test]
    fn owns_only_del_roots_and_descendants_and_deduplicates_ports() {
        let mut rows = parse(
            "10 1 S /app/del\n11 10 S /bin/zsh\n12 11 R /bin/node\n13 12 S /bin/worker\n20 1 S /bin/private\n30 1 S /bin/claude\n31 30 S /bin/helper\n40 10 R /bin/ps\n41 10 Z (git)\n42 10 R (git)\n43 11 S /usr/bin/git\n44 30 S /bin/node\n45 44 S /bin/node_repl",
            &[
                Root {
                    pid: 30,
                    owner: Some("chat:a".into()),
                },
                Root {
                    pid: 11,
                    owner: Some("chat:b".into()),
                },
            ],
            10,
        );
        assert_eq!(
            rows.iter().map(|r| r.pid).collect::<Vec<_>>(),
            [11, 12, 13, 30, 31, 43, 44, 45]
        );
        add_ports(
            &mut rows,
            "p12\nn127.0.0.1:3000\nn[::1]:3000\nn*:8080\np20\nn*:9999",
        );
        assert_eq!(rows[1].ports, ["3000", "8080"]);
        assert_eq!(rows[1].owner.as_deref(), Some("chat:b"));
        assert_eq!(rows[4].owner.as_deref(), Some("chat:a"));
        assert!(rows[0].background); // terminal shell
        assert!(rows[3].background); // provider connection
        assert!(!rows[1].background); // user/agent-started command
        assert_eq!(rows.iter().filter(|r| !r.background).count(), 4);
    }
}
