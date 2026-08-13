use crate::level::Level;
use chopped_asset_handler::fetch_asset_bytes;

pub struct LevelManager {
    list: LevelList,
    /// Index into `list.list` of the level that is currently loaded into the game world.
    current_index: usize,
}

#[derive(serde::Deserialize)]
pub struct LevelList {
    list: Vec<String>,
}

impl LevelManager {
    pub async fn new(list_path: &str) -> Self {
        let bytes = fetch_asset_bytes(list_path, env!("CARGO_MANIFEST_DIR"))
            .await
            .expect("should be able to fetch config.ron");
        let list: LevelList = ron::de::from_bytes(&bytes).expect("level file should be valid RON");
        assert!(
            !list.list.is_empty(),
            "level list should contain at least one level"
        );
        Self {
            list,
            current_index: 0,
        }
    }

    pub fn get_current_index(&self) -> usize {
        self.current_index
    }

    // The name (without the `.level` extension) of the level we're currently on.
    pub fn current_level_name(&self) -> &str {
        &self.list.list[self.current_index]
    }

    // Reads the level we're currently on off disk.
    pub async fn load_current_level(&self) -> Level {
        Level::create_from_file(self.current_level_name()).await
    }

    // Reads the level we're currently on off disk.
    pub async fn load_level_by_index(&mut self, index: usize) -> Level {
        self.current_index = index;
        Level::create_from_file(&self.list.list[self.current_index]).await
    }

    /// Moves us onto the next level in the list, wrapping back around to the
    /// first one once the player exits the last level.
    pub fn advance(&mut self) {
        self.current_index = (self.current_index + 1) % self.list.list.len();
    }

    pub fn is_on_last_level(&self) -> bool {
        self.current_index == self.list.list.len().saturating_sub(1)
    }
}
