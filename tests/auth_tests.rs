mod common;

use common::spawn_app;
use reqwest::Client;
use serde_json::json;

#[tokio::test]
async fn register_works() {
    let app = spawn_app().await;
    let client = Client::new();

    let response = client
        .post(&format!("{}/api/auth/register", app.address))
        .json(&json!({
            "username": "testuser",
            "password": "password123"
        }))
        .send()
        .await
        .expect("Failed to execute request");

    assert!(response.status().is_success());
}

#[tokio::test]
async fn login_works() {
    let app = spawn_app().await;
    let client = Client::builder().cookie_store(true).build().unwrap();

    // Register first
    client
        .post(&format!("{}/api/auth/register", app.address))
        .json(&json!({
            "username": "testuser",
            "password": "password123"
        }))
        .send()
        .await
        .expect("Failed to execute request");

    // Login
    let response = client
        .post(&format!("{}/api/auth/login", app.address))
        .json(&json!({
            "username": "testuser",
            "password": "password123"
        }))
        .send()
        .await
        .expect("Failed to execute request");

    assert!(response.status().is_success());
}