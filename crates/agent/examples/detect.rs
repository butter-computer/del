//! `cargo run -p del-agent --example detect` prints the local CLI status as JSON.
#[tokio::main(flavor = "current_thread")]
async fn main() {
    println!(
        "{}",
        serde_json::to_string_pretty(&del_agent::detect_all().await).unwrap()
    );
}
