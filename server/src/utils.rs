use std::collections::HashMap;

use crate::errors::ServerError;
use barreleye_common::models::PrimaryId;

pub fn extract_primary_ids(
	field: &str,
	mut ids: Vec<String>,
	map: HashMap<String, PrimaryId>,
) -> Result<Vec<PrimaryId>, ServerError> {
	if !ids.is_empty() {
		ids.sort_unstable();
		ids.dedup();

		let invalid_ids = ids
			.into_iter()
			.filter_map(|tag_id| if !map.contains_key(&tag_id) { Some(tag_id) } else { None })
			.collect::<Vec<String>>();

		if !invalid_ids.is_empty() {
			return Err(ServerError::InvalidValues {
				field: field.to_string(),
				values: invalid_ids.join(", "),
			});
		}

		Ok(map.into_values().collect())
	} else {
		Ok(vec![])
	}
}
