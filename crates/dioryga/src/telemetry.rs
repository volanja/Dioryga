//! ログの初期化。

use tracing_subscriber::EnvFilter;

/// ログを初期化する。
///
/// フィルタは環境変数 `RUST_LOG` を優先し、無ければ設定値を使う。
pub fn init(default_filter: &str) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}
