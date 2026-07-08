//! Native stdio entry point for the Solcore language server.

#[cfg(feature = "native")]
#[tokio::main]
async fn main() {
    solcore_lsp::native::run_stdio().await;
}

#[cfg(not(feature = "native"))]
fn main() {
    eprintln!("solcore-lsp server requires the `native` feature");
}
