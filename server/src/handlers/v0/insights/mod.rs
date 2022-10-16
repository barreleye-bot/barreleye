use axum::{routing::get, Router};
use std::sync::Arc;

use crate::ServerState;

mod get;

pub fn get_routes(shared_state: Arc<ServerState>) -> Router<Arc<ServerState>> {
	Router::with_state(shared_state).route("/", get(get::handler))
}
