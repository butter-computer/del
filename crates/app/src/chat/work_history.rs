//! Presentation ranges keep intermediate events intact for expansion and search.
use super::model::{Block, ChatModel};
use std::ops::Range;
use std::time::{Duration, Instant};

/// One reversible reveal shared by every row in a work section.
#[derive(Clone, Copy)]
pub struct Reveal {
    pub started: Instant,
    from: f32,
    to: f32,
}

impl Reveal {
    pub fn new(from: f32, open: bool, started: Instant) -> Self {
        Self {
            started,
            from,
            to: if open { 1. } else { 0. },
        }
    }

    pub fn sample(self, now: Instant) -> (f32, bool) {
        let duration = if self.to == 1. { 240. } else { 180. };
        let t =
            (now.saturating_duration_since(self.started).as_secs_f32() * 1000. / duration).min(1.);
        let eased = 1. - (1. - t).powi(3);
        (self.from + (self.to - self.from) * eased, t >= 1.)
    }
}

pub fn duration_label(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds >= 3600 {
        format!(
            "Worked for {}h {}m {}s",
            seconds / 3600,
            seconds % 3600 / 60,
            seconds % 60
        )
    } else if seconds >= 60 {
        format!("Worked for {}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("Worked for {seconds}s")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub start: usize,
    pub end: usize,
    pub answer: usize,
}

impl Section {
    pub fn range(&self) -> Range<usize> {
        self.start..self.end
    }
}

pub fn request_start(model: &ChatModel, before: usize) -> usize {
    model.blocks[..before]
        .iter()
        .rposition(|block| {
            matches!(
                block,
                Block::User {
                    steer: false,
                    goal: false,
                    ..
                }
            )
        })
        .map_or(0, |ix| ix + 1)
}

/// Each section starts after a user prompt and retains the final answer outside
/// its disclosure, including when restored peer receipts trail that answer.
/// Peer continuations stay in that range, even across provider turns.
/// Never fold the current task while it is running, waiting, paused, or failed.
pub fn sections(model: &ChatModel, work_pending: bool) -> Vec<Section> {
    let mut result = Vec::new();
    let mut start = 0;
    for end in 0..=model.blocks.len() {
        let boundary = matches!(
            model.blocks.get(end),
            Some(Block::User {
                steer: false,
                goal: false,
                ..
            })
        );
        if end != model.blocks.len() && !boundary {
            continue;
        }
        let current = end == model.blocks.len();
        if start < end && !(current && (work_pending || model.is_running())) {
            // Peer receipt updates may trail an answer. Other trailing activity
            // means this is not yet the final answer (including errors/stops).
            if let Some(answer) = (start..end)
                .rev()
                .find(|&ix| !matches!(model.blocks[ix], Block::Peer(_)))
                && model.is_completed_response(answer)
                && matches!(&model.blocks[answer], Block::Assistant { text, open: false, .. } if !text.trim().is_empty())
            {
                result.push(Section { start, end, answer });
            }
        }
        start = end + 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use del_agent::{AgentEvent, TurnStatus};

    #[test]
    fn reveal_reverses_without_jumping_and_finishes() {
        let now = Instant::now();
        let opening = Reveal::new(0., true, now);
        assert_eq!(opening.sample(now), (0., false));
        let midway = now + Duration::from_millis(80);
        let progress = opening.sample(midway).0;
        assert!(progress > 0. && progress < 1.);
        let closing = Reveal::new(progress, false, midway);
        assert_eq!(closing.sample(midway).0, progress);
        assert_eq!(
            closing.sample(midway + Duration::from_millis(180)),
            (0., true)
        );
        assert_eq!(opening.sample(now + Duration::from_millis(240)), (1., true));
    }

    #[test]
    fn elapsed_labels_keep_minutes_seconds_and_hours() {
        assert_eq!(duration_label(Duration::from_secs(69)), "Worked for 1m 9s");
        assert_eq!(duration_label(Duration::from_secs(7)), "Worked for 7s");
        assert_eq!(
            duration_label(Duration::from_secs(3602)),
            "Worked for 1h 0m 2s"
        );
    }

    fn answer(model: &mut ChatModel, id: &str, text: &str) {
        model.apply(&AgentEvent::TurnStarted { turn_id: id.into() });
        model.apply(&AgentEvent::AssistantMessage {
            turn_id: id.into(),
            text: text.into(),
        });
        model.apply(&AgentEvent::TurnCompleted {
            turn_id: id.into(),
            status: TurnStatus::Completed,
        });
    }

    #[test]
    fn peer_turns_stay_visible_until_final_answer_and_work_complete() {
        let mut m = ChatModel::default();
        m.push_user("delegate".into(), false);
        answer(&mut m, "one", "Waiting for workers");
        assert!(sections(&m, true).is_empty());
        answer(&mut m, "two", "Two replies arrived");
        assert!(sections(&m, true).is_empty());
        m.apply(&AgentEvent::TurnStarted {
            turn_id: "final".into(),
        });
        m.apply(&AgentEvent::AssistantTextDelta {
            turn_id: "final".into(),
            text: "All done".into(),
        });
        assert!(
            sections(&m, false).is_empty(),
            "work complete before final stream ends"
        );
        m.apply(&AgentEvent::TurnCompleted {
            turn_id: "final".into(),
            status: TurnStatus::Completed,
        });
        assert_eq!(
            sections(&m, false),
            vec![Section {
                start: 1,
                end: 4,
                answer: 3
            }]
        );
        // Only the last row gets a footer, even when the work is expanded.
        assert_eq!(
            sections(&m, false)
                .iter()
                .map(|r| r.answer)
                .collect::<Vec<_>>(),
            vec![3]
        );
    }

    #[test]
    fn previous_answers_remain_separate_while_next_task_runs() {
        let mut m = ChatModel::default();
        m.push_user("first".into(), false);
        answer(&mut m, "one", "First final");
        m.push_user("second".into(), false);
        m.apply(&AgentEvent::TurnStarted {
            turn_id: "two".into(),
        });
        assert_eq!(
            sections(&m, true),
            vec![Section {
                start: 1,
                end: 2,
                answer: 1
            }]
        );
        m.apply(&AgentEvent::AssistantMessage {
            turn_id: "two".into(),
            text: "Second final".into(),
        });
        m.apply(&AgentEvent::TurnCompleted {
            turn_id: "two".into(),
            status: TurnStatus::Completed,
        });
        assert_eq!(
            sections(&m, false),
            vec![
                Section {
                    start: 1,
                    end: 2,
                    answer: 1
                },
                Section {
                    start: 3,
                    end: 4,
                    answer: 3
                }
            ]
        );
    }

    #[test]
    fn restored_peer_receipts_after_the_answer_belong_to_its_disclosure() {
        let mut m = ChatModel::default();
        m.push_user("task".into(), false);
        answer(&mut m, "one", "Final");
        m.apply(&AgentEvent::PeerMessage {
            message: del_agent::peers::PeerMessage {
                id: "receipt".into(),
                idempotency_key: "key".into(),
                from: del_agent::SessionId::new("a"),
                to: del_agent::SessionId::new("b"),
                reply_to: None,
                text: "task".into(),
                created_at_ms: 0,
                state: del_agent::peers::DeliveryState::Delivered,
                error: None,
            },
        });
        assert_eq!(
            sections(&m, false),
            vec![Section {
                start: 1,
                end: 3,
                answer: 1
            }]
        );
    }

    #[test]
    fn failed_and_cancelled_work_never_disappears() {
        for status in [TurnStatus::Failed, TurnStatus::Interrupted] {
            let mut m = ChatModel::default();
            m.push_user("task".into(), false);
            answer(&mut m, "one", "Still working");
            m.apply(&AgentEvent::TurnStarted {
                turn_id: "two".into(),
            });
            m.apply(&AgentEvent::TurnCompleted {
                turn_id: "two".into(),
                status,
            });
            assert!(sections(&m, false).is_empty());
        }
    }
}
