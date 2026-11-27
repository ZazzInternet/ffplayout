use sqlx::sqlite::{SqlitePool, SqliteQueryResult};

use crate::{db::models::WebPage, utils::errors::ProcessError};

pub async fn select_web_pages(
    pool: &SqlitePool,
    channel_id: i32,
) -> Result<Vec<WebPage>, ProcessError> {
    const QUERY: &str = "SELECT * FROM config_web_pages WHERE channel_id = $1 ORDER BY name, id";

    Ok(sqlx::query_as(QUERY)
        .bind(channel_id)
        .fetch_all(pool)
        .await?)
}

pub async fn select_web_page(
    pool: &SqlitePool,
    channel_id: i32,
    id: i32,
) -> Result<Option<WebPage>, ProcessError> {
    const QUERY: &str = "SELECT * FROM config_web_pages WHERE channel_id = $1 AND id = $2";

    Ok(sqlx::query_as(QUERY)
        .bind(channel_id)
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

pub async fn insert_web_page(pool: &SqlitePool, page: WebPage) -> Result<WebPage, ProcessError> {
    const QUERY: &str = "INSERT INTO config_web_pages
        (channel_id, name, url, steps, audio, zoom, preroll)
        VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING *";

    Ok(sqlx::query_as(QUERY)
        .bind(page.channel_id)
        .bind(page.name)
        .bind(page.url)
        .bind(page.steps)
        .bind(page.audio)
        .bind(page.zoom)
        .bind(page.preroll)
        .fetch_one(pool)
        .await?)
}

pub async fn update_web_page(
    pool: &SqlitePool,
    channel_id: i32,
    id: i32,
    page: WebPage,
) -> Result<SqliteQueryResult, ProcessError> {
    const QUERY: &str = "UPDATE config_web_pages
        SET name = $1, url = $2, steps = $3, audio = $4, zoom = $5, preroll = $6
        WHERE id = $7 AND channel_id = $8";

    Ok(sqlx::query(QUERY)
        .bind(page.name)
        .bind(page.url)
        .bind(page.steps)
        .bind(page.audio)
        .bind(page.zoom)
        .bind(page.preroll)
        .bind(id)
        .bind(channel_id)
        .execute(pool)
        .await?)
}

pub async fn delete_web_page(
    pool: &SqlitePool,
    channel_id: i32,
    id: i32,
) -> Result<SqliteQueryResult, ProcessError> {
    const QUERY: &str = "DELETE FROM config_web_pages WHERE id = $1 AND channel_id = $2";

    Ok(sqlx::query(QUERY)
        .bind(id)
        .bind(channel_id)
        .execute(pool)
        .await?)
}

#[cfg(test)]
mod tests {
    use sqlx::{sqlite::SqlitePoolOptions, types::Json};

    use super::*;
    use crate::db::{
        handles::db_migrate,
        models::{WebStep, WebStepAction},
    };

    #[tokio::test]
    async fn web_page_crud_is_scoped_to_the_channel() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        db_migrate(&pool).await.unwrap();

        let page = WebPage {
            name: "Dashboard".to_string(),
            url: "https://example.org".to_string(),
            steps: Json(vec![WebStep {
                action: WebStepAction::Click,
                selector: "#login".to_string(),
                ..WebStep::default()
            }]),
            ..WebPage::default()
        };
        let stored = insert_web_page(&pool, page.clone()).await.unwrap();

        assert!(stored.id > 0);
        assert_eq!(stored.steps.0, page.steps.0);
        assert_eq!(select_web_pages(&pool, 1).await.unwrap().len(), 1);
        assert!(select_web_pages(&pool, 999).await.unwrap().is_empty());
        assert!(
            select_web_page(&pool, 999, stored.id)
                .await
                .unwrap()
                .is_none()
        );

        let renamed = WebPage {
            name: "Scoreboard".to_string(),
            ..page
        };

        assert_eq!(
            update_web_page(&pool, 999, stored.id, renamed.clone())
                .await
                .unwrap()
                .rows_affected(),
            0
        );
        assert_eq!(
            update_web_page(&pool, 1, stored.id, renamed)
                .await
                .unwrap()
                .rows_affected(),
            1
        );
        assert_eq!(
            select_web_page(&pool, 1, stored.id)
                .await
                .unwrap()
                .unwrap()
                .name,
            "Scoreboard"
        );
        assert_eq!(
            delete_web_page(&pool, 999, stored.id)
                .await
                .unwrap()
                .rows_affected(),
            0
        );
        assert_eq!(
            delete_web_page(&pool, 1, stored.id)
                .await
                .unwrap()
                .rows_affected(),
            1
        );
    }
}
