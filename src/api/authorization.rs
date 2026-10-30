use super::{
    auth::Claims,
    error::{ApiError, ApiResult},
};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Role {
    #[serde(rename = "admin")]
    Admin,
    #[serde(rename = "dokter")]
    Doctor,
    #[serde(rename = "pasien")]
    Patient,
}
impl Role {
    pub fn parse(value: &str) -> ApiResult<Self> {
        match value {
            "admin" => Ok(Self::Admin),
            "dokter" => Ok(Self::Doctor),
            "pasien" => Ok(Self::Patient),
            _ => Err(ApiError::forbidden()),
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Doctor => "dokter",
            Self::Patient => "pasien",
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Identity {
    #[serde(skip)]
    pub access_token: Option<String>,
    pub auth_id: String,
    pub actor_id: String,
    pub account_id: String,
    pub profile_id: String,
    pub role: Role,
    #[serde(skip)]
    pub claims: Claims,
    #[serde(skip)]
    pub impersonation_id: Option<String>,
}
impl Identity {
    pub async fn load(pool: &PgPool, claims: &Claims) -> ApiResult<Self> {
        let row = sqlx::query(
            "SELECT id,role,tokens_valid_after FROM accounts WHERE auth_id=$1 AND enabled=true",
        )
        .bind(&claims.sub)
        .fetch_optional(pool)
        .await?
        .ok_or_else(ApiError::forbidden)?;
        let role = Role::parse(row.get("role"))?;
        let profile_id = Self::profile_for_account(pool, row.get("id"), &role).await?;
        if (claims.iat as i64) < row.get::<i64, _>("tokens_valid_after") {
            return Err(ApiError::unauthorized());
        }
        let id: String = row.get("id");
        Ok(Self {
            claims: claims.clone(),
            access_token: None,
            impersonation_id: None,
            auth_id: claims.sub.clone(),
            actor_id: id.clone(),
            account_id: id,
            profile_id,
            role,
        })
    }
    pub async fn profile_for_account(
        pool: &PgPool,
        account: &str,
        role: &Role,
    ) -> ApiResult<String> {
        let table = match role {
            Role::Admin => return Ok(account.to_owned()),
            Role::Patient => "patients",
            Role::Doctor => "doctors",
        };
        let statement = format!("SELECT id FROM {table} WHERE account_id=$1 LIMIT 2");
        let profiles: Vec<String> = sqlx::query_scalar(&statement)
            .bind(account)
            .fetch_all(pool)
            .await?;
        if profiles.len() > 1 {
            return Err(ApiError::conflict("Account has multiple profiles; administrator review is required before login or impersonation"));
        }
        profiles.into_iter().next().ok_or_else(ApiError::forbidden)
    }
    pub fn admin(&self) -> ApiResult<()> {
        if self.role == Role::Admin {
            Ok(())
        } else {
            Err(ApiError::forbidden())
        }
    }
    pub async fn patient(&self, pool: &PgPool, id: &str) -> ApiResult<String> {
        let row = sqlx::query(
            "SELECT id,account_id,primary_doctor_id FROM patients WHERE id=$1 OR account_id=$1",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(ApiError::missing)?;
        let allowed = self.role == Role::Admin
            || (self.role == Role::Patient
                && row.get::<Option<String>, _>("account_id").as_deref() == Some(&self.account_id))
            || (self.role == Role::Doctor
                && row.get::<Option<String>, _>("primary_doctor_id").as_deref()
                    == Some(&self.profile_id));
        if !allowed {
            return Err(ApiError::forbidden());
        }
        Ok(row.get("id"))
    }
    pub async fn doctor(&self, pool: &PgPool, id: &str) -> ApiResult<String> {
        let row = sqlx::query("SELECT id,account_id FROM doctors WHERE id=$1 OR account_id=$1")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .ok_or_else(ApiError::missing)?;
        let doctor_id: String = row.get("id");
        if self.role == Role::Admin
            || row.get::<Option<String>, _>("account_id").as_deref() == Some(&self.account_id)
        {
            return Ok(doctor_id);
        }
        if self.role == Role::Patient {
            let connected:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM patients WHERE account_id=$1 AND primary_doctor_id=$2)").bind(&self.account_id).bind(&doctor_id).fetch_one(pool).await?;
            if connected {
                return Ok(doctor_id);
            }
        }
        Err(ApiError::forbidden())
    }
    pub async fn session(&self, pool: &PgPool, id: &str) -> ApiResult<String> {
        validate_id(id)?;
        let patient: String = sqlx::query_scalar(
            "SELECT patient_id FROM sessions WHERE id=$1 AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(ApiError::missing)?;
        self.patient(pool, &patient).await?;
        Ok(patient)
    }
    pub async fn device(&self, pool: &PgPool, id: &str) -> ApiResult<String> {
        let device: String = sqlx::query_scalar("SELECT id FROM devices WHERE id=$1 OR name=$1")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .ok_or_else(ApiError::missing)?;
        if self.role == Role::Admin {
            return Ok(device);
        }
        let patients: Vec<String> =
            sqlx::query_scalar("SELECT id FROM patients WHERE device_id=$1")
                .bind(&device)
                .fetch_all(pool)
                .await?;
        for patient in patients {
            if self.patient(pool, &patient).await.is_ok() {
                return Ok(device);
            }
        }
        Err(ApiError::forbidden())
    }
}
pub fn validate_id(id: &str) -> ApiResult<()> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        Err(ApiError::bad("Invalid resource ID"))
    } else {
        Ok(())
    }
}
