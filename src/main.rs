#[tokio::main]
async fn main() -> std::io::Result<()> {
    mailroom::server::run().await
}
