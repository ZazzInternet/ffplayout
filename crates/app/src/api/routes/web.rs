use axum::{
    Json,
    extract::{Path, State},
};
use protect_axum::authorities::{AuthDetails, AuthoritiesCheck};

use crate::{
    api::{
        routes::{AuthUser, ensure_any_authority},
        state::AppState,
    },
    db::{
        handles,
        models::{Role, WebPage},
    },
    player::{output::web_geometry, web::WebPageTest},
    utils::errors::ServiceError,
};

/// Loading a page lets the server browse a URL and shows the result, and its
/// steps can hold credentials, so only administrators manage web pages.
const MANAGE_ROLES: [&Role; 2] = [&Role::GlobalAdmin, &Role::ChannelAdmin];

/// #### Web Pages
///
/// Web pages can be scheduled in a playlist with the source `web://<id>`.
///
/// **Get all web pages of a channel**
///
/// Credentials in steps are only returned to administrators.
///
/// ```BASH
/// curl -X GET http://127.0.0.1:8787/api/web/1 -H 'Authorization: Bearer <TOKEN>'
/// ```
pub async fn get_web_pages(
    State(state): State<AppState>,
    Path(channel): Path<i32>,
    user: AuthUser,
    details: AuthDetails<Role>,
) -> Result<Json<Vec<WebPage>>, ServiceError> {
    ensure_any_authority(
        &details,
        &[&Role::GlobalAdmin, &Role::ChannelAdmin, &Role::User],
    )?;
    user.ensure_channel_or_admin(channel)?;

    let mut pages = handles::select_web_pages(&state.pool, channel)
        .await
        .map_err(|_| ServiceError::InternalServerError)?;

    if !details.has_any_authority(&MANAGE_ROLES) {
        for step in pages.iter_mut().flat_map(|page| page.steps.iter_mut()) {
            if step.secret {
                step.value.clear();
            }
        }
    }

    Ok(Json(pages))
}

/// **Add a web page**
///
/// ```BASH
/// curl -X POST http://127.0.0.1:8787/api/web/1 -H 'Content-Type: application/json' \
/// -d '{ "name": "Dashboard", "url": "https://example.org/board", "audio": true, "zoom": 1.0, "preroll": 20,
///       "steps": [ { "action": "type", "selector": "#user", "value": "operator" },
///                  { "action": "type", "selector": "#password", "value": "secret", "secret": true },
///                  { "action": "click", "selector": "button[type=submit]" },
///                  { "action": "wait_for", "selector": "#board", "seconds": 20 } ] }' \
/// -H 'Authorization: Bearer <TOKEN>'
/// ```
pub async fn add_web_page(
    State(state): State<AppState>,
    Path(channel): Path<i32>,
    user: AuthUser,
    details: AuthDetails<Role>,
    Json(mut data): Json<WebPage>,
) -> Result<Json<WebPage>, ServiceError> {
    ensure_any_authority(&details, &MANAGE_ROLES)?;
    user.ensure_channel_or_admin(channel)?;
    data.channel_id = channel;
    data.validate().map_err(ServiceError::BadRequest)?;

    handles::insert_web_page(&state.pool, data)
        .await
        .map(Json)
        .map_err(|_| ServiceError::InternalServerError)
}

/// **Update a web page**
///
/// ```BASH
/// curl -X PUT http://127.0.0.1:8787/api/web/1/3 -H 'Content-Type: application/json' \
/// -d '{ "name": "Dashboard", "url": "https://example.org/board", "steps": [] }' \
/// -H 'Authorization: Bearer <TOKEN>'
/// ```
pub async fn update_web_page(
    State(state): State<AppState>,
    Path((channel, id)): Path<(i32, i32)>,
    user: AuthUser,
    details: AuthDetails<Role>,
    Json(mut data): Json<WebPage>,
) -> Result<&'static str, ServiceError> {
    ensure_any_authority(&details, &MANAGE_ROLES)?;
    user.ensure_channel_or_admin(channel)?;
    data.channel_id = channel;
    data.validate().map_err(ServiceError::BadRequest)?;

    match handles::update_web_page(&state.pool, channel, id, data).await {
        Ok(result) if result.rows_affected() == 1 => Ok("Update Success"),
        Ok(_) => Err(not_found(channel, id)),
        Err(_) => Err(ServiceError::InternalServerError),
    }
}

/// **Delete a web page**
///
/// ```BASH
/// curl -X DELETE http://127.0.0.1:8787/api/web/1/3 -H 'Authorization: Bearer <TOKEN>'
/// ```
pub async fn delete_web_page(
    State(state): State<AppState>,
    Path((channel, id)): Path<(i32, i32)>,
    user: AuthUser,
    details: AuthDetails<Role>,
) -> Result<&'static str, ServiceError> {
    ensure_any_authority(&details, &MANAGE_ROLES)?;
    user.ensure_channel_or_admin(channel)?;

    match handles::delete_web_page(&state.pool, channel, id).await {
        Ok(result) if result.rows_affected() == 1 => Ok("Delete Success"),
        Ok(_) => Err(not_found(channel, id)),
        Err(_) => Err(ServiceError::InternalServerError),
    }
}

/// **Test a web page**
///
/// Loads the page and runs its steps once without putting it on air. The
/// response reports a failing step and contains a screenshot (base64 JPEG).
///
/// ```BASH
/// curl -X POST http://127.0.0.1:8787/api/web/1/test -H 'Content-Type: application/json' \
/// -d '{ "name": "Dashboard", "url": "https://example.org/board", "steps": [] }' \
/// -H 'Authorization: Bearer <TOKEN>'
/// ```
pub async fn test_web_page(
    State(state): State<AppState>,
    Path(channel): Path<i32>,
    user: AuthUser,
    details: AuthDetails<Role>,
    Json(mut data): Json<WebPage>,
) -> Result<Json<WebPageTest>, ServiceError> {
    ensure_any_authority(&details, &MANAGE_ROLES)?;
    user.ensure_channel_or_admin(channel)?;
    data.channel_id = channel;
    data.validate().map_err(ServiceError::BadRequest)?;

    let manager = {
        let guard = state.controller.read().await;
        guard.get(channel)
    }
    .ok_or_else(|| ServiceError::BadRequest(format!("Channel {channel} not found!")))?;
    let geometry = web_geometry(&*manager.config.read().await);

    Ok(Json(manager.web.test(&data, geometry).await))
}

fn not_found(channel: i32, id: i32) -> ServiceError {
    ServiceError::NotFound(format!("Web page {id} not found for channel {channel}"))
}
