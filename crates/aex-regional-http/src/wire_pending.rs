//! Temporary trait seam until generated server traits are merged.

// TODO(cross-stream): replaced by aex_wire::server::RegionalSessionApi at merge
/// Generated finite-session server trait pending the contracts merge.
pub trait RegionalSessionApi {
    /// Router containing only fully served session routes.
    fn router(self) -> axum::Router;
}

// TODO(cross-stream): replaced by aex_wire::server::RegionalSecretApi at merge
/// Generated plaintext-secret server trait pending the contracts merge.
pub trait RegionalSecretApi {
    /// Router containing only fully served secret routes.
    fn router(self) -> axum::Router;
}

// TODO(cross-stream): replaced by aex_wire::server::RegionalStreamApi at merge
/// Generated regional-stream server trait pending the contracts merge.
pub trait RegionalStreamApi {
    /// Router containing only fully served stream routes.
    fn router(self) -> axum::Router;
}
