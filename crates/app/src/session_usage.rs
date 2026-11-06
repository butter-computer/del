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
    /// Reconcile each conversation separately before adding it to the family.
    /// Legacy coverage floors and provider-native scopes never overlap siblings.
    pub fn with_side_chats<'a>(
        &self,
        sides: impl Iterator<Item = &'a BTreeMap<String, f64>>,
    ) -> Self {
        let mut family = Self {
            context_tokens: self.context_tokens,
            context_window: self.context_window,
            ..Default::default()
        };
        for (index, costs) in std::iter::once(self.costs.clone())
            .chain(sides.cloned())
            .enumerate()
        {
            let usage = Self {
                costs,
                ..Default::default()
            };
            for (provider, (cost, estimated)) in usage.provider_costs() {
                let estimate = if estimated { "estimate:" } else { "" };
                family.costs.insert(
                    format!("v2:conversation-{index}:{estimate}{provider}:total"),
                    cost,
                );
            }
        }
        family
    }
    #[cfg(test)]
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
        let mut lines = vec!["Main + side chats · all provider switches".into()];
        for (provider, cost) in self.provider_costs() {
            lines.push(format!(
                "{}  {}",
                provider_name(&provider),
                money(Some(cost))
            ));
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
/// A provider's cost-ledger key as people know it: `codex` is "Codex".
pub fn provider_name(binary: &str) -> &str {
    match binary {
        "claude" => "Claude",
        "codex" => "Codex",
        "opencode" => "OpenCode",
        other => other,
    }
}
/// "$1,234.56", with " est." when any part is estimated.
pub fn money(cost: Option<(f64, bool)>) -> String {
    match cost {
        Some((value, estimated)) => {
            let suffix = if estimated { " est." } else { "" };
            if value > 0. && value < 0.01 {
                format!("<$0.01{suffix}")
            } else {
                let cents = (value.max(0.) * 100.).round() as u64;
                format!(
                    "${}.{:02}{suffix}",
                    crate::ui::group_digits(cents / 100),
                    cents % 100
                )
            }
        }
        None => "—".into(),
    }
}
pub(crate) fn merge_cost(costs: &mut BTreeMap<String, f64>, scope: &str, cost: f64) {
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
    fn costs_read_as_money_with_grouped_dollars() {
        assert_eq!(money(Some((1234.564, true))), "$1,234.56 est.");
        assert_eq!(money(Some((1234567.5, false))), "$1,234,567.50");
        assert_eq!(money(Some((0.999, false))), "$1.00");
        assert_eq!(money(Some((0., false))), "$0.00");
        assert_eq!(money(Some((0.004, true))), "<$0.01 est.");
        assert_eq!(money(None), "—");
        assert_eq!(provider_name("codex"), "Codex");
        let mut usage = SessionUsage::default();
        merge_cost(&mut usage.costs, "codex:thread", 2.);
        assert!(usage.cost_breakdown().contains("Codex  $2.00"));
        assert!(!usage.cost_breakdown().contains("OpenAI"));
    }
    #[test]
    fn family_costs_add_siblings_and_provider_switches_without_cross_chat_floors() {
        let main = SessionUsage {
            context_tokens: Some(100),
            context_window: Some(1000),
            costs: BTreeMap::from([("claude:legacy".into(), 5.0)]),
        };
        let sides = [
            BTreeMap::from([
                ("v2:a:claude:run".into(), 2.0),
                ("v2:a:estimate:claude:run".into(), 1.0),
                ("v2:b:estimate:codex:thread".into(), 3.0),
                ("v2:c:claude:run".into(), 4.0),
            ]),
            BTreeMap::from([("claude:legacy".into(), 6.0)]),
        ];
        let family = main.with_side_chats(sides.iter());
        assert_eq!(family.provider_costs()["claude"], (17.0, false));
        assert_eq!(family.provider_costs()["codex"], (3.0, true));
        assert_eq!(family.cost_summary(), Some((20.0, true)));
        assert_eq!(family.context_label(), "Context 10% used");
        assert_eq!(
            main.with_side_chats(std::iter::empty()).total_cost(),
            Some(5.0)
        );
    }

    #[test]
    fn side_costs_survive_replay_provider_changes_closing_and_owner_migration() {
        let mut saved = crate::sessions::Overrides::default();
        let draft = SessionId::new("draft:0");
        let parent = SessionId::new("main");
        let mut first = BTreeMap::from([("v2:a:claude:run".into(), 2.0)]);
        assert!(saved.merge_side_chat_costs(&draft, "side-one", &first));
        assert!(!saved.merge_side_chat_costs(&draft, "side-one", &first));
        first.insert("v2:b:estimate:codex:run".into(), 3.0);
        assert!(saved.merge_side_chat_costs(&draft, "side-one", &first));
        saved.merge_side_chat_costs(
            &draft,
            "side-two",
            &BTreeMap::from([("v2:a:claude:run".into(), 7.0)]),
        );
        saved.move_side_chat_state(&draft, &parent);
        // Closing a tab clears its unread state, not the cost already incurred.
        saved.set_side_chat_unread(&parent, "side-one", false);
        let bytes = serde_json::to_vec(&saved).unwrap();
        let mut restored: crate::sessions::Overrides = serde_json::from_slice(&bytes).unwrap();
        let stale = BTreeMap::from([("v2:a:claude:run".into(), 1.0)]);
        assert!(!restored.merge_side_chat_costs(&parent, "side-one", &stale));
        let family = SessionUsage::default().with_side_chats(restored.side_chat_costs(&parent));
        assert_eq!(family.cost_summary(), Some((12.0, true)));
        assert_eq!(restored.side_chat_costs(&draft).count(), 0);
        assert_eq!(
            restored.side_chat_costs(&SessionId::new("other")).count(),
            0
        );
    }

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
