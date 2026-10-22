//! Canned transcript for `DEL_BACKEND=mock-demo`: every block type at once, so
//! the whole page can be reviewed without driving scenarios by hand.

use del_agent::{
    AgentEvent as E, ApprovalKind, FileChangeKind, ToolStatus,
};

const MARKDOWN: &str = "Here is the plan:\n\n1. Read `main.rs`\n2. Add a **run** function\n\n```rust\nfn run() {\n    println!(\"hello\");\n}\n```\n\nSee [the docs](https://example.com) for more.";

pub fn events() -> Vec<E> {
    let t = || "demo".to_string();
    vec![
        E::TurnStarted { turn_id: t() },
        E::ReasoningDelta {
            turn_id: t(),
            text: "The user wants a run function. I should read the entry point first, then edit it.".into(),
        },
        E::AssistantMessage { turn_id: t(), text: MARKDOWN.into() },
        E::ToolCallStarted { id: "c1".into(), name: "read_file".into(), input_summary: "src/main.rs".into() },
        E::ToolCallFinished {
            id: "c1".into(),
            status: ToolStatus::Completed,
            output_summary: "fn main() {}\n\n42 lines read".into(),
        },
        E::ToolCallStarted { id: "c2".into(), name: "shell".into(), input_summary: "cargo test --all".into() },
        E::ToolCallFinished {
            id: "c2".into(),
            status: ToolStatus::Failed,
            output_summary: "error[E0425]: cannot find function `run`".into(),
        },
        E::ToolCallStarted { id: "c3".into(), name: "grep".into(), input_summary: "\"fn run\" src/".into() },
        E::FileChange {
            path: "src/main.rs".into(),
            kind: FileChangeKind::Modified,
            diff: Some("@@ -1,3 +1,5 @@\n fn main() {\n-    todo!()\n+    run();\n }\n+\n+fn run() {}\n".into()),
        },
        E::FileChange { path: "src/new.rs".into(), kind: FileChangeKind::Created, diff: None },
        E::ApprovalRequested {
            id: "a1".into(),
            kind: ApprovalKind::Command,
            detail: "rm -rf build/".into(),
        },
        E::Error { message: "Rate limited; retrying in 5s".into(), recoverable: true },
        E::TokenUsage { input: 1200, output: 340, cached: 800 },
    ]
}
