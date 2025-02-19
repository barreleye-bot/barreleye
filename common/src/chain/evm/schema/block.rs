use duckdb::{params, Connection};
use ethers::{
	abi::AbiEncode,
	types::{H160, H256, U256},
};
use eyre::Result;

use super::ParquetFile;
use crate::storage::StorageModelTrait;

#[derive(Debug)]
pub struct Block {
	pub hash: Option<H256>,
	pub parent_hash: H256,
	pub author: Option<H160>,
	pub state_root: H256,
	pub transactions_root: H256,
	pub receipts_root: H256,
	pub number: Option<u64>,
	pub gas_used: U256,
	pub timestamp: u64,
	pub total_difficulty: Option<U256>,
	pub base_fee_per_gas: Option<U256>,
}

impl StorageModelTrait for Block {
	fn create_table(&self, db: &Connection) -> Result<()> {
		db.execute_batch(&format!(
			r#"CREATE TEMP TABLE IF NOT EXISTS {} (
                hash BLOB,
                parent_hash BLOB NOT NULL,
                author BLOB,
                state_root BLOB NOT NULL,
                transactions_root BLOB NOT NULL,
                receipts_root BLOB NOT NULL,
                number UINT64,
                gas_used VARCHAR NOT NULL,
                timestamp UINT64,
                total_difficulty VARCHAR,
                base_fee_per_gas VARCHAR
            );"#,
			ParquetFile::Block
		))?;

		Ok(())
	}

	fn insert(&self, db: &Connection) -> Result<()> {
		self.create_table(db)?;

		db.execute(
			&format!(
				r#"INSERT INTO {} (
                    hash,
                    parent_hash,
                    author,
                    state_root,
                    transactions_root,
                    receipts_root,
                    number,
                    gas_used,
                    timestamp,
                    total_difficulty,
                    base_fee_per_gas
                ) VALUES (
                    ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?
                );"#,
				ParquetFile::Block
			),
			params![
				self.hash.map(|v| v.encode()),
				self.parent_hash.encode(),
				self.author.map(|v| v.encode()),
				self.state_root.encode(),
				self.transactions_root.encode(),
				self.receipts_root.encode(),
				self.number,
				self.gas_used.to_string(),
				self.timestamp,
				self.total_difficulty.map(|v| v.to_string()),
				self.base_fee_per_gas.map(|v| v.to_string()),
			],
		)?;

		Ok(())
	}
}
