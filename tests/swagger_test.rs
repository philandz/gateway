use axum::Router;
use gateway::{swagger, AppState};
use std::sync::Arc;

#[test]
fn test_swagger_router_builds_successfully() {
    let _r: Router<Arc<AppState>> = swagger::router();
}
