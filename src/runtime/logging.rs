use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

pub(super) fn setup_logging(config: &crate::config::Config) -> WorkerGuard {
    let file_appender = tracing_appender::rolling::daily(&config.log.dir, &config.log.file);
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let level: tracing::Level = config.log.level.parse().unwrap_or(tracing::Level::INFO);

    tracing_subscriber::registry()
        .with(EnvFilter::from_default_env().add_directive(level.into()))
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::fmt::layer().with_writer(non_blocking))
        .init();

    guard
}
