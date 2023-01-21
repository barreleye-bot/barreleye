use async_trait::async_trait;
use eyre::Result;
use sea_orm::{
	entity::prelude::*,
	query::*,
	sea_query::{types::*, Expr, SimpleExpr},
	ActiveValue, QuerySelect,
};

pub use self::config::{Config, ConfigKey};
use crate::utils;
pub use address::{Address, AddressActiveModel};
pub use amount::Amount;
pub use api_key::{ApiKey, ApiKeyActiveModel};
pub use balance::Balance;
pub use entity::{LabeledEntity as Entity, LabeledEntityActiveModel as EntityActiveModel};
pub use entity_tags::EntityTags;
pub use link::{Link, LinkUuid};
pub use network::{Network, NetworkActiveModel};
pub use relation::{Reason as RelationReason, Relation};
pub use tag::{Tag, TagActiveModel};
pub use transfer::Transfer;

pub mod address;
pub mod amount;
pub mod api_key;
pub mod balance;
pub mod config;
pub mod entity;
pub mod entity_tags;
pub mod link;
pub mod network;
pub mod relation;
pub mod tag;
pub mod transfer;

// @TODO `https://github.com/SeaQL/sea-orm/issues/1068`
pub type PrimaryId = i64;

pub fn set<T>(v: T) -> ActiveValue<T>
where
	T: Into<sea_orm::Value>,
{
	ActiveValue::set(v)
}

pub fn optional_set<T>(o: Option<T>) -> ActiveValue<T>
where
	T: Into<sea_orm::Value>,
{
	match o {
		Some(v) => ActiveValue::set(v),
		_ => ActiveValue::not_set(),
	}
}

#[async_trait]
pub trait BasicModel {
	type ActiveModel: ActiveModelTrait + ActiveModelBehavior + Sized + Send;

    async fn create<C>(
		c: &C,
		data: Self::ActiveModel,
	) -> Result<<<<Self::ActiveModel as ActiveModelTrait>::Entity as EntityTrait>::PrimaryKey as
	PrimaryKeyTrait>::ValueType>
	where
		C: ConnectionTrait,
	{
		let insert_result =
			<Self::ActiveModel as ActiveModelTrait>::Entity::insert(data).exec(c).await?;

		Ok(insert_result.last_insert_id)
	}

    async fn create_many<C>(
		c: &C,
		data: Vec<Self::ActiveModel>,
	) -> Result<<<<Self::ActiveModel as ActiveModelTrait>::Entity as EntityTrait>::PrimaryKey as
	PrimaryKeyTrait>::ValueType>
	where
		C: ConnectionTrait,
	{
		let insert_result =
			<Self::ActiveModel as ActiveModelTrait>::Entity::insert_many(data).exec(c).await?;

		Ok(insert_result.last_insert_id)
	}

	async fn get<C>(
		c: &C,
		primary_id: <<<Self::ActiveModel as ActiveModelTrait>::Entity as EntityTrait>::PrimaryKey as
		PrimaryKeyTrait>::ValueType,
	) -> Result<Option<<<Self::ActiveModel as ActiveModelTrait>::Entity as EntityTrait>::Model>>
	where
		C: ConnectionTrait,
	{
		Ok(<Self::ActiveModel as ActiveModelTrait>::Entity::find_by_id(primary_id).one(c).await?)
	}

	async fn get_by_id<C>(
		c: &C,
		id: &str,
	) -> Result<Option<<<Self::ActiveModel as ActiveModelTrait>::Entity as EntityTrait>::Model>>
	where
		C: ConnectionTrait,
	{
		Ok(<Self::ActiveModel as ActiveModelTrait>::Entity::find()
			.filter(Expr::col(Alias::new("id")).eq(id))
			.one(c)
			.await?)
	}

	async fn get_all_by_ids<C>(
		c: &C,
		ids: Vec<String>,
	) -> Result<Vec<<<Self::ActiveModel as ActiveModelTrait>::Entity as EntityTrait>::Model>>
	where
		C: ConnectionTrait,
	{
		Ok(<Self::ActiveModel as ActiveModelTrait>::Entity::find()
			.filter(Expr::col(Alias::new("id")).is_in(ids))
			.all(c)
			.await?)
	}

	async fn get_all<C>(
		c: &C,
	) -> Result<Vec<<<Self::ActiveModel as ActiveModelTrait>::Entity as EntityTrait>::Model>>
	where
		C: ConnectionTrait,
	{
		Ok(Self::get_all_where(c, vec![], None, None).await?)
	}

	async fn get_all_where<C>(
		c: &C,
		conditions: Vec<SimpleExpr>,
		offset: Option<u64>,
		limit: Option<u64>,
	) -> Result<Vec<<<Self::ActiveModel as ActiveModelTrait>::Entity as EntityTrait>::Model>>
	where
		C: ConnectionTrait,
	{
		let mut filter = Condition::all();
		for condition in conditions.into_iter() {
			filter = filter.add(condition);
		}

		let mut q = <Self::ActiveModel as ActiveModelTrait>::Entity::find().filter(filter);

		if let Some(v) = offset {
			q = q.offset(v);
		}

		if let Some(v) = limit {
			q = q.limit(v);
		}

		Ok(q.all(c).await?)
	}

	async fn update_by_id<C>(c: &C, id: &str, data: Self::ActiveModel) -> Result<bool>
	where
		C: ConnectionTrait,
	{
		let res = <Self::ActiveModel as ActiveModelTrait>::Entity::update_many()
			.col_expr(Alias::new("updated_at"), Expr::value(utils::now()))
			.set(data)
			.filter(Expr::col(Alias::new("id")).eq(id))
			.exec(c)
			.await?;

		Ok(res.rows_affected == 1)
	}

	async fn delete<C>(
		c: &C,
		primary_id: <<<Self::ActiveModel as ActiveModelTrait>::Entity as EntityTrait>::PrimaryKey as
		PrimaryKeyTrait>::ValueType,
	) -> Result<bool>
	where
		C: ConnectionTrait,
	{
		let res = <Self::ActiveModel as ActiveModelTrait>::Entity::delete_by_id(primary_id)
			.exec(c)
			.await?;

		Ok(res.rows_affected == 1)
	}

	async fn delete_by_id<C>(c: &C, id: &str) -> Result<bool>
	where
		C: ConnectionTrait,
	{
		let res = <Self::ActiveModel as ActiveModelTrait>::Entity::delete_many()
			.filter(Expr::col(Alias::new("id")).eq(id))
			.exec(c)
			.await?;

		Ok(res.rows_affected == 1)
	}

	async fn delete_by_ids<C>(c: &C, ids: Vec<String>) -> Result<u64>
	where
		C: ConnectionTrait,
	{
		let res = <Self::ActiveModel as ActiveModelTrait>::Entity::delete_many()
			.filter(Expr::col(Alias::new("id")).is_in(ids))
			.exec(c)
			.await?;

		Ok(res.rows_affected)
	}
}

#[async_trait]
pub trait SoftDeleteModel {
	type ActiveModel: ActiveModelTrait + ActiveModelBehavior + Sized + Send;

	async fn get_existing_by_id<C>(
		c: &C,
		id: &str,
	) -> Result<Option<<<Self::ActiveModel as ActiveModelTrait>::Entity as EntityTrait>::Model>>
	where
		C: ConnectionTrait,
	{
		Ok(<Self::ActiveModel as ActiveModelTrait>::Entity::find()
			.filter(Expr::col(Alias::new("id")).eq(id))
			.filter(Expr::col(Alias::new("is_deleted")).eq(false))
			.one(c)
			.await?)
	}

	async fn get_all_deleted<C>(
		c: &C,
	) -> Result<Vec<<<Self::ActiveModel as ActiveModelTrait>::Entity as EntityTrait>::Model>>
	where
		C: ConnectionTrait,
	{
		Ok(<Self::ActiveModel as ActiveModelTrait>::Entity::find()
			.filter(Expr::col(Alias::new("is_deleted")).eq(true))
			.all(c)
			.await?)
	}

	async fn prune_all<C>(c: &C) -> Result<u64>
	where
		C: ConnectionTrait,
	{
		let res = <Self::ActiveModel as ActiveModelTrait>::Entity::delete_many()
			.filter(Expr::col(Alias::new("is_deleted")).eq(true))
			.exec(c)
			.await?;

		Ok(res.rows_affected)
	}

	async fn prune_all_by_ids<C>(c: &C, ids: Vec<String>) -> Result<u64>
	where
		C: ConnectionTrait,
	{
		let res = <Self::ActiveModel as ActiveModelTrait>::Entity::delete_many()
			.filter(Expr::col(Alias::new("is_deleted")).eq(true))
			.filter(Expr::col(Alias::new("id")).is_in(ids))
			.exec(c)
			.await?;

		Ok(res.rows_affected)
	}
}
