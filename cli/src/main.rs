#[tokio::main]
async fn main() {
    if let Err(error) = aem_cli::run().await {
        eprintln!(
            "aem: {} ({})",
            aem_cli::safe_terminal_text(&error.message),
            aem_cli::safe_terminal_text(&error.code)
        );
        std::process::exit(1);
    }
}
