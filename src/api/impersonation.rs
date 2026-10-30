use super::{
    authorization::{Identity, Role},
    error::{ApiError, ApiResult},
    routes::AppState,
};
use axum::{
    extract::{Path, State},
    routing::{delete, post},
    Extension, Json, Router,
};
use serde_json::{json, Value};
use sqlx::Row;
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/admin/impersonate/:id", post(start))
        .route("/api/doctors/impersonate/:id", post(start))
        .route("/api/impersonation/:id", delete(stop))
}
async fn start(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(target): Path<String>,
) -> ApiResult<Json<Value>> {
    if actor.actor_id != actor.account_id {
        return Err(ApiError::forbidden());
    }
    let target_row =
        sqlx::query("SELECT a.id,a.role FROM accounts a WHERE a.id=$1 AND a.enabled=true")
            .bind(&target)
            .fetch_one(&state.pool)
            .await?;
    let role = Role::parse(target_row.get("role"))?;
    let profile_id =
        Identity::profile_for_account(&state.pool, target_row.get("id"), &role).await?;
    if role == Role::Admin {
        return Err(ApiError::forbidden());
    }
    if actor.role == Role::Doctor {
        if role != Role::Patient {
            return Err(ApiError::forbidden());
        }
        actor.patient(&state.pool, &profile_id).await?;
    } else {
        actor.admin()?
    }
    let id = crate::db::postgres::new_id("imp");
    sqlx::query("INSERT INTO impersonation_sessions(id,actor_id,effective_id,expires_at) VALUES($1,$2,$3,now()+interval '10 minutes')")
  .bind(&id).bind(&actor.account_id).bind(&target).execute(&state.pool).await?;
    Ok(Json(
        json!({"success":true,"impersonation_id":id,"user_id":target,"account_id":target,"profile_id":profile_id,"role":role,"expires_in":600}),
    ))
}
pub async fn resolve(state: &AppState, actor: &Identity, id: &str) -> ApiResult<Identity> {
    let row=sqlx::query("SELECT a.id,a.role FROM impersonation_sessions i JOIN accounts a ON a.id=i.effective_id WHERE i.id=$1 AND i.actor_id=$2 AND i.revoked_at IS NULL AND i.expires_at>now() AND a.enabled=true")
  .bind(id).bind(&actor.account_id).fetch_optional(&state.pool).await?.ok_or_else(ApiError::forbidden)?;
    let role = Role::parse(row.get("role"))?;
    let profile_id = Identity::profile_for_account(&state.pool, row.get("id"), &role).await?;
    if role == Role::Admin {
        return Err(ApiError::forbidden());
    }
    if actor.role == Role::Doctor {
        actor.patient(&state.pool, &profile_id).await?;
    } else {
        actor.admin()?
    }
    Ok(Identity {
        account_id: row.get("id"),
        profile_id,
        role,
        ..actor.clone()
    })
}
async fn stop(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let result = sqlx::query(
        "UPDATE impersonation_sessions SET revoked_at=now() WHERE id=$1 AND actor_id=$2",
    )
    .bind(id)
    .bind(identity.actor_id)
    .execute(&state.pool)
    .await?;
    if result.rows_affected() != 1 {
        return Err(ApiError::missing());
    }
    Ok(Json(json!({"success":true})))
}
