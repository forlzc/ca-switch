use std::sync::Arc;
use std::time::Duration;

use tauri::Manager;

use super::startup_tests::TestHome;
use crate::database::Database;
use crate::provider::Provider;
use crate::store::AppState;

