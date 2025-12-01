mod common;

use common::spawn_app;
use reqwest::Client;
use serde_json::json;

#[tokio::test]
async fn list_files_requires_auth() {
    let app = spawn_app().await;
    let client = Client::new();

    let response = client
        .get(&format!("{}/api/files", app.address))
        .send()
        .await
        .expect("Failed to execute request");

    assert_eq!(response.status().as_u16(), 401);
}

#[tokio::test]
async fn list_files_works_with_auth() {
    let app = spawn_app().await;
    let client = Client::builder().cookie_store(true).build().unwrap();

    // Register and Login
    client
        .post(&format!("{}/api/auth/register", app.address))
        .json(&json!({
            "username": "testuser",
            "password": "password123"
        }))
        .send()
        .await
        .expect("Failed to register");

    client
        .post(&format!("{}/api/auth/login", app.address))
        .json(&json!({
            "username": "testuser",
            "password": "password123"
        }))
        .send()
        .await
        .expect("Failed to login");

    // List files
    let response = client
        .get(&format!("{}/api/files", app.address))
        .send()
        .await
        .expect("Failed to execute request");

    assert!(response.status().is_success());
}