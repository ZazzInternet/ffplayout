use super::*;

fn web_request(
    method: &str,
    uri: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> Request<Body> {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"));

    match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

async fn json_body(response: axum::response::Response) -> serde_json::Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();

    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
#[serial]
async fn web_page_routes_cover_crud_validation_and_permissions() {
    let fixture = security_fixture().await;
    let user = access_token(&fixture.state, 1).await;
    let admin = access_token(&fixture.state, 2).await;
    let page = json!({
        "name": "Dashboard",
        "url": "https://example.org/board",
        "steps": [
            {"action": "type", "selector": "#user", "value": "operator"},
            {"action": "type", "selector": "#password", "value": "secret", "secret": true},
            {"action": "click", "selector": "button[type=submit]"},
        ],
    });

    // Regular users may neither create nor test pages.
    for uri in ["/api/web/1", "/api/web/1/test"] {
        let response = fixture
            .app
            .clone()
            .oneshot(web_request("POST", uri, &user, Some(page.clone())))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{uri}");
    }

    for invalid in [
        json!({"name": "", "url": "https://example.org"}),
        json!({"name": "Local file", "url": "file:///etc/passwd"}),
        json!({"name": "No selector", "url": "https://example.org", "steps": [{"action": "click"}]}),
    ] {
        let response = fixture
            .app
            .clone()
            .oneshot(web_request("POST", "/api/web/1", &admin, Some(invalid)))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    let created = fixture
        .app
        .clone()
        .oneshot(web_request(
            "POST",
            "/api/web/1",
            &admin,
            Some(page.clone()),
        ))
        .await
        .unwrap();

    assert_eq!(created.status(), StatusCode::OK);

    let created = json_body(created).await;
    let id = created["id"].as_i64().unwrap();

    assert_eq!(created["channel_id"], 1);
    assert_eq!(created["preroll"], 20);

    // Administrators read credentials back; regular users only see the page.
    for (token, password) in [(&admin, "secret"), (&user, "")] {
        let listed = fixture
            .app
            .clone()
            .oneshot(web_request("GET", "/api/web/1", token, None))
            .await
            .unwrap();

        assert_eq!(listed.status(), StatusCode::OK);

        let listed = json_body(listed).await;

        assert_eq!(listed[0]["name"], "Dashboard");
        assert_eq!(listed[0]["steps"][0]["value"], "operator");
        assert_eq!(listed[0]["steps"][1]["value"], password);
    }

    // A page belongs to one channel.
    let other_channel = fixture
        .app
        .clone()
        .oneshot(web_request("GET", "/api/web/2", &admin, None))
        .await
        .unwrap();

    assert_eq!(json_body(other_channel).await, json!([]));

    let mut renamed = page.clone();

    renamed["name"] = json!("Scoreboard");

    for (uri, status) in [
        (format!("/api/web/2/{id}"), StatusCode::NOT_FOUND),
        (format!("/api/web/1/{id}"), StatusCode::OK),
    ] {
        let response = fixture
            .app
            .clone()
            .oneshot(web_request("PUT", &uri, &admin, Some(renamed.clone())))
            .await
            .unwrap();

        assert_eq!(response.status(), status, "{uri}");
    }

    let name: String = sqlx::query_scalar("SELECT name FROM config_web_pages WHERE id = $1")
        .bind(id)
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();

    assert_eq!(name, "Scoreboard");

    for (token, uri, status) in [
        (&user, format!("/api/web/1/{id}"), StatusCode::FORBIDDEN),
        (&admin, format!("/api/web/2/{id}"), StatusCode::NOT_FOUND),
        (&admin, format!("/api/web/1/{id}"), StatusCode::OK),
        (&admin, format!("/api/web/1/{id}"), StatusCode::NOT_FOUND),
    ] {
        let response = fixture
            .app
            .clone()
            .oneshot(web_request("DELETE", &uri, token, None))
            .await
            .unwrap();

        assert_eq!(response.status(), status, "{uri}");
    }
}
