//! Display-only provider telemetry. Context is a snapshot; costs are cumulative
//! per provider process/step, never a sum of repeated notifications.
use del_agent::{AgentEvent, SessionId};
use std::collections::BTreeMap;

#[derive(Clone, Default)]
pub struct SessionUsage {
    pub context_tokens: Option<u64>,
    pub context_window: Option<u64>,
    pub costs: BTreeMap<String, f64>,
}
impl SessionUsage {
    pub fn apply(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::ProviderSwitched { .. } | AgentEvent::SessionStarted { .. } => {
                self.context_tokens = None;
                self.context_window = None;
            }
            AgentEvent::UsageSnapshot {
                context_tokens,
                context_window,
                cost_scope,
                cost_usd,
            } => {
                if context_tokens.is_some() {
                    self.context_tokens = *context_tokens;
                    // A new request without a capacity must not inherit a stale model limit.
                    self.context_window = context_window.filter(|n| *n > 0);
                }
                if context_window.is_some() {
                    self.context_window = context_window.filter(|n| *n > 0);
                }
                if let (Some(scope), Some(cost)) = (cost_scope, cost_usd) {
                    merge_cost(&mut self.costs, scope, *cost);
                }
            }
            _ => {}
        }
    }
    /// Reconcile overlapping costs within each native provider-switch segment.
    /// OpenCode reports per-step values, so disjoint reported/estimated steps add.
    pub fn provider_costs(&self) -> BTreeMap<String, (f64, bool)> {
        let mut groups: BTreeMap<(String, String, String), (f64, f64, bool)> = BTreeMap::new();
        let scoped_aliases: std::collections::HashSet<&str> = self
            .costs
            .keys()
            .filter_map(|key| key.strip_prefix("v2:")?.split_once(':').map(|(_, raw)| raw))
            .collect();
        for (key, cost) in &self.costs {
            let (segment, raw) = match key.strip_prefix("v2:").and_then(|key| key.split_once(':')) {
                Some((segment, raw)) => (segment, raw),
                None if scoped_aliases.contains(key.as_str()) => continue,
                None => ("legacy", key.as_str()),
            };
            let (raw, estimated) = raw
                .strip_prefix("estimate:")
                .map_or((raw, false), |s| (s, true));
            let (provider, unit) = raw.split_once(':').unwrap_or((raw, ""));
            let unit = if provider == "opencode" { unit } else { "" };
            let totals = groups
                .entry((provider.into(), segment.into(), unit.into()))
                .or_default();
            if estimated {
                totals.1 += cost;
            } else {
                totals.0 += cost;
                totals.2 = true;
            }
        }
        let mut known: BTreeMap<String, (f64, bool)> = BTreeMap::new();
        let mut legacy: BTreeMap<String, (f64, bool)> = BTreeMap::new();
        for ((provider, segment, _), (reported, estimate, has_reported)) in groups {
            let totals = if segment == "legacy" {
                &mut legacy
            } else {
                &mut known
            };
            let total = totals.entry(provider).or_default();
            total.0 += if has_reported && reported >= estimate {
                reported
            } else {
                estimate
            };
            total.1 |= !has_reported || estimate > reported;
        }
        // Old ledgers have no segment identity. They may overlap replayed history;
        // use them only as a coverage floor, never add them to the new ledger.
        for (provider, total) in legacy {
            let value = known.entry(provider).or_insert(total);
            if total.0 > value.0 {
                *value = (total.0, true);
            }
        }
        known.retain(|_, cost| cost.0.is_finite());
        known
    }
    pub fn provider_cost(&self, provider: del_agent::Provider) -> Option<(f64, bool)> {
        self.provider_costs().get(provider.binary()).copied()
    }
    pub fn cost_summary(&self) -> Option<(f64, bool)> {
        let costs = self.provider_costs();
        (!costs.is_empty()).then(|| {
            (
                costs.values().map(|c| c.0).sum(),
                costs.values().any(|c| c.1),
            )
        })
    }
    pub fn cost_breakdown(&self) -> String {
        let mut lines = vec!["This conversation · all provider switches".into()];
        for (provider, cost) in self.provider_costs() {
            let name = match provider.as_str() {
                "claude" => "Claude",
                "codex" => "OpenAI / Codex",
                "opencode" => "OpenCode",
                other => other,
            };
            lines.push(format!("{name}  {}", money(Some(cost))));
        }
        if let Some(total) = self.cost_summary() {
            lines.push(format!("Total  {}", money(Some(total))));
        } else {
            lines.push("No priced usage captured yet".into());
        }
        lines.push("Estimates may exclude unreported or Terminal usage".into());
        lines.join("\n")
    }
    #[cfg(test)]
    pub fn total_cost(&self) -> Option<f64> {
        self.cost_summary().map(|c| c.0)
    }
    pub fn context_label(&self) -> String {
        match (self.context_tokens, self.context_window) {
            (Some(used), Some(limit)) if limit > 0 => format!(
                "Context {:.0}% used",
                (used as f64 / limit as f64 * 100.).clamp(0., 100.)
            ),
            (Some(_), _) => "Context —".into(),
            _ => "Context —".into(),
        }
    }
}
pub fn money(cost: Option<(f64, bool)>) -> String {
    match cost {
        Some((value, estimated)) => {
            let suffix = if estimated { " (est)" } else { "" };
            if value > 0. && value < 0.01 {
                format!("<$0.01{suffix}")
            } else {
                format!("${value:.2}{suffix}")
            }
        }
        None => "—".into(),
    }
}
fn merge_cost(costs: &mut BTreeMap<String, f64>, scope: &str, cost: f64) {
    if cost.is_finite() && cost >= 0. {
        costs
            .entry(scope.to_owned())
            .and_modify(|old| *old = old.max(cost))
            .or_insert(cost);
    }
}
// Background-only file access. Serial merging prevents an older save or a second
// view of this conversation from overwriting newer captured costs.
static LEDGER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn path(id: &SessionId) -> Option<std::path::PathBuf> {
    let name: String = id.as_str().bytes().map(|b| format!("{b:02x}")).collect();
    Some(
        del_agent::data_dir()?
            .join("usage")
            .join(format!("{name}.json")),
    )
}
pub fn load_costs(id: &SessionId) -> BTreeMap<String, f64> {
    let _lock = LEDGER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    path(id)
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}
pub fn remove(id: &SessionId) {
    let _lock = LEDGER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(path) = path(id) {
        let _ = std::fs::remove_file(path);
    }
}
pub fn save_cost(id: &SessionId, scope: &str, cost: f64) -> std::io::Result<()> {
    let _lock = LEDGER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(path) = path(id) else {
        return Ok(());
    };
    save_cost_at(&path, scope, cost)
}
fn save_cost_at(path: &std::path::Path, scope: &str, cost: f64) -> std::io::Result<()> {
    use std::io::Write;
    let mut costs = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    merge_cost(&mut costs, scope, cost);
    let parent = path.parent().expect("usage directory");
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&serde_json::to_vec(&costs)?)?;
    temporary.persist(path).map_err(|e| e.error)?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_provider_visits_reconcile_per_segment_and_survive_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cost.json");
        // Claude -> OpenCode -> Claude -> OpenCode -> Claude -> Codex.
        for (session, scope, cost) in [
            ("a", "estimate:claude:msg-a", 0.5),
            ("a", "claude:run-a", 0.7),
            ("b", "opencode:step-b", 0.2),
            ("c", "estimate:claude:msg-c", 0.4),
            ("d", "estimate:opencode:step-d", 0.3),
            ("d", "opencode:step-d2", 0.6),
            ("e", "estimate:claude:msg-e", 0.1),
            ("e", "claude:run-e", 0.2),
            ("f", "estimate:codex:thread-f", 0.9),
            ("a", "claude:run-a", 0.7),            // duplicate replay
            ("f", "estimate:codex:thread-f", 0.1), // stale cumulative event
        ] {
            save_cost_at(
                &path,
                &del_agent::pricing::cost_scope(Some(session), scope.into()),
                cost,
            )
            .unwrap();
        }
        let usage = SessionUsage {
            costs: serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap(),
            ..Default::default()
        };
        let costs = usage.provider_costs();
        assert!((costs["claude"].0 - 1.3).abs() < 1e-9);
        assert!((costs["opencode"].0 - 1.1).abs() < 1e-9);
        assert_eq!(costs["codex"], (0.9, true));
        assert!((usage.total_cost().unwrap() - 3.3).abs() < 1e-9);
        assert_eq!(
            usage.provider_cost(del_agent::Provider::Claude),
            Some(costs["claude"])
        );
    }
    #[test]
    fn migrated_scope_does_not_duplicate_legacy_usage() {
        let mut usage = SessionUsage::default();
        merge_cost(&mut usage.costs, "estimate:claude:msg", 0.5);
        merge_cost(
            &mut usage.costs,
            &del_agent::pricing::cost_scope(Some("a"), "estimate:claude:msg".into()),
            0.5,
        );
        assert_eq!(usage.total_cost(), Some(0.5));
    }
    #[test]
    fn estimates_do_not_double_count_reported_cost_and_sum_across_providers() {
        let mut usage = SessionUsage::default();
        merge_cost(&mut usage.costs, "estimate:claude:message", 0.3);
        merge_cost(&mut usage.costs, "claude:process", 0.4);
        merge_cost(&mut usage.costs, "estimate:codex:thread", 0.2);
        assert!((usage.total_cost().unwrap() - 0.6).abs() < 1e-9);
        assert!(usage.cost_summary().unwrap().1);
        merge_cost(&mut usage.costs, "codex:thread", 0.25);
        assert!(!usage.cost_summary().unwrap().1);
    }
    #[test]
    fn persisted_costs_merge_across_restarts_and_out_of_order_saves() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cost.json");
        save_cost_at(&path, "a", 0.4).unwrap();
        save_cost_at(&path, "b", 0.2).unwrap();
        save_cost_at(&path, "a", 0.1).unwrap();
        let costs: BTreeMap<String, f64> =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(costs.len(), 2);
        assert_eq!(costs["a"], 0.4);
        assert_eq!(costs["b"], 0.2);
    }
    #[test]
    fn repeated_costs_replace_and_provider_switch_resets_context_only() {
        let mut usage = SessionUsage::default();
        for cost in [0.2, 0.4, 0.3, 0.4] {
            usage.apply(&AgentEvent::UsageSnapshot {
                context_tokens: Some(50),
                context_window: Some(100),
                cost_scope: Some("a".into()),
                cost_usd: Some(cost),
            });
        }
        assert_eq!(usage.total_cost(), Some(0.4));
        assert_eq!(usage.context_label(), "Context 50% used");
        usage.apply(&AgentEvent::ProviderSwitched {
            from: del_agent::Provider::Claude,
            to: del_agent::Provider::Codex,
            session_id: SessionId::new("new"),
            at_ms: 0,
        });
        assert_eq!(usage.context_label(), "Context —");
        usage.apply(&AgentEvent::UsageSnapshot {
            context_tokens: Some(20),
            context_window: None,
            cost_scope: Some("b".into()),
            cost_usd: Some(0.1),
        });
        assert_eq!(usage.total_cost(), Some(0.5));
        assert_eq!(usage.context_label(), "Context —");
    }
    #[test]
    fn missing_cost_is_unknown_and_invalid_cost_is_rejected() {
        let mut usage = SessionUsage::default();
        for cost in [f64::NAN, f64::INFINITY, -1.] {
            merge_cost(&mut usage.costs, "a", cost);
        }
        assert_eq!(usage.total_cost(), None);
        merge_cost(&mut usage.costs, "a", 0.);
        assert_eq!(usage.total_cost(), Some(0.));
    }
}
