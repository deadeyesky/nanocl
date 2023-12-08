use std::sync::Arc;

use diesel::prelude::*;
use tokio::task::JoinHandle;

use nanocl_error::io::{IoError, IoResult};

use nanocl_stubs::generic::GenericFilter;
use nanocl_stubs::resource::ResourceSpec;

use crate::schema::resource_specs;
use crate::{utils, gen_where4string, gen_where4json};

use super::{Pool, Repository, ResourceDb};

/// This structure represent the resource spec in the database.
/// A resource spec represent the specification of a resource.
/// It is stored as a json object in the database.
/// We use the `resource_key` to link to the resource.
#[derive(Clone, Queryable, Identifiable, Insertable, Associations)]
#[diesel(primary_key(key))]
#[diesel(table_name = resource_specs)]
#[diesel(belongs_to(ResourceDb, foreign_key = resource_key))]
pub struct ResourceSpecDb {
  /// The key of the resource spec
  pub key: uuid::Uuid,
  /// The created at date
  pub created_at: chrono::NaiveDateTime,
  /// The resource key reference
  pub resource_key: String,
  /// The version of the resource spec
  pub version: String,
  /// The data of the spec
  pub data: serde_json::Value,
  /// The metadata (user defined)
  pub metadata: Option<serde_json::Value>,
}

/// Helper to convert a `ResourceSpecDb` to a `ResourceSpec`
impl From<ResourceSpecDb> for ResourceSpec {
  fn from(db: ResourceSpecDb) -> Self {
    ResourceSpec {
      key: db.key,
      version: db.version,
      created_at: db.created_at,
      resource_key: db.resource_key,
      data: db.data,
      metadata: db.metadata,
    }
  }
}

impl Repository for ResourceSpecDb {
  type Table = resource_specs::table;
  type Item = ResourceSpecDb;
  type UpdateItem = ResourceSpecDb;

  fn find_one(
    filter: &GenericFilter,
    pool: &Pool,
  ) -> JoinHandle<IoResult<Self::Item>> {
    log::debug!("ResourceSpecDb::find_one filter: {filter:?}");
    let r#where = filter.r#where.to_owned().unwrap_or_default();
    let mut query = resource_specs::dsl::resource_specs.into_boxed();
    if let Some(value) = r#where.get("version") {
      gen_where4string!(query, resource_specs::dsl::version, value);
    }
    if let Some(value) = r#where.get("resource_key") {
      gen_where4string!(query, resource_specs::dsl::resource_key, value);
    }
    if let Some(value) = r#where.get("data") {
      gen_where4json!(query, resource_specs::dsl::data, value);
    }
    if let Some(value) = r#where.get("metadata") {
      gen_where4json!(query, resource_specs::dsl::metadata, value);
    }
    let pool = Arc::clone(pool);
    ntex::rt::spawn_blocking(move || {
      let mut conn = utils::store::get_pool_conn(&pool)?;
      let item = query
        .get_result::<Self>(&mut conn)
        .map_err(Self::map_err_context)?;
      Ok::<_, IoError>(item)
    })
  }

  fn find(
    filter: &GenericFilter,
    pool: &Pool,
  ) -> JoinHandle<IoResult<Vec<Self::Item>>> {
    log::debug!("ResourceSpecDb::find filter: {filter:?}");
    let r#where = filter.r#where.to_owned().unwrap_or_default();
    let mut query = resource_specs::dsl::resource_specs.into_boxed();
    if let Some(value) = r#where.get("version") {
      gen_where4string!(query, resource_specs::dsl::version, value);
    }
    if let Some(value) = r#where.get("resource_key") {
      gen_where4string!(query, resource_specs::dsl::resource_key, value);
    }
    if let Some(value) = r#where.get("data") {
      gen_where4json!(query, resource_specs::dsl::data, value);
    }
    if let Some(value) = r#where.get("metadata") {
      gen_where4json!(query, resource_specs::dsl::metadata, value);
    }
    let pool = Arc::clone(pool);
    ntex::rt::spawn_blocking(move || {
      let mut conn = utils::store::get_pool_conn(&pool)?;
      let items = query
        .get_results::<Self>(&mut conn)
        .map_err(Self::map_err_context)?;
      Ok::<_, IoError>(items)
    })
  }
}