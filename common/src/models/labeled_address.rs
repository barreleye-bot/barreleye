use eyre::Result;
use sea_orm::entity::{prelude::*, *};
use sea_orm_migration::prelude::OnConflict;
use serde::{Deserialize, Serialize};

use crate::{
	models::{label, BasicModel, PrimaryId},
	utils, Address, Db, IdPrefix,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, DeriveEntityModel)]
#[sea_orm(table_name = "labeled_addresses")]
#[serde(rename_all = "camelCase")]
pub struct Model {
	#[sea_orm(primary_key)]
	#[serde(skip_serializing, skip_deserializing)]
	pub labeled_address_id: PrimaryId,
	#[serde(skip_serializing)]
	pub label_id: PrimaryId,
	pub id: String,
	pub address: String,
	#[sea_orm(nullable)]
	#[serde(skip_serializing)]
	pub updated_at: Option<DateTime>,
	pub created_at: DateTime,
}

pub use ActiveModel as LabeledAddressActiveModel;
pub use Model as LabeledAddress;

#[derive(Copy, Clone, Debug, EnumIter)]
pub enum Relation {
	Label,
}

impl RelationTrait for Relation {
	fn def(&self) -> RelationDef {
		match self {
			Self::Label => Entity::belongs_to(label::Entity)
				.from(Column::LabelId)
				.to(label::Column::LabelId)
				.into(),
		}
	}
}

impl ActiveModelBehavior for ActiveModel {}

impl BasicModel for Model {
	type ActiveModel = ActiveModel;
}

impl Model {
	pub fn new_model(label_id: PrimaryId, address: Address) -> ActiveModel {
		ActiveModel {
			label_id: Set(label_id),
			id: Set(utils::new_unique_id(IdPrefix::LabeledAddress)),
			address: Set(address.to_string()),
			..Default::default()
		}
	}

	pub async fn create_many(db: &Db, data: Vec<ActiveModel>) -> Result<PrimaryId> {
		let insert_result = Entity::insert_many(data)
			.on_conflict(
				OnConflict::columns([Column::LabelId, Column::Address])
					// @TODO this should be a `.do_nothing()`, but: `https://github.com/SeaQL/sea-orm/issues/899`
					.update_column(Column::LabeledAddressId)
					.to_owned(),
			)
			.exec(db.get())
			.await?;

		Ok(insert_result.last_insert_id)
	}

	pub async fn get_all_by_label_ids(db: &Db, label_ids: Vec<PrimaryId>) -> Result<Vec<Self>> {
		Ok(Entity::find().filter(Column::LabelId.is_in(label_ids)).all(db.get()).await?)
	}

	pub async fn get_by_address(db: &Db, address: &str) -> Result<Option<Self>> {
		Ok(Entity::find().filter(Column::Address.eq(address.to_lowercase())).one(db.get()).await?)
	}

	pub async fn get_all_by_label_id_and_addresses(
		db: &Db,
		label_id: PrimaryId,
		addresses: Vec<String>,
	) -> Result<Vec<Self>> {
		Ok(Entity::find()
			.filter(Column::LabelId.eq(label_id))
			.filter(Column::Address.is_in(addresses.into_iter().map(|a| a.to_lowercase())))
			.all(db.get())
			.await?)
	}
}
