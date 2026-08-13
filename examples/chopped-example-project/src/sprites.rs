use crate::sprite_list::SpriteList;
use chopped_asset_handler::fetch_asset_bytes;
use chopped_engine::kiss3d::prelude::*;

pub async fn preload_sprites(
    manifest_path: &str,
    texture_manager: &mut TextureManager,
    manifest_dir: &str,
) {
    println!("manifest_path: {manifest_path}, manifest_dir: {manifest_dir}");
    let bytes = fetch_asset_bytes(manifest_path, manifest_dir)
        .await
        .expect("should be able to fetch the sprite manifest");
    let manifest: SpriteList =
        ron::de::from_bytes(&bytes).expect("sprite manifest should be valid RON");

    for (name, entry) in &manifest.sprites {
        let image_bytes = fetch_asset_bytes(&entry.path, manifest_dir)
            .await
            .unwrap_or_else(|e| {
                panic!("failed to fetch sprite '{name}' from '{}': {e}", entry.path)
            });

        texture_manager.add_image_from_memory_pixelated(&image_bytes, name);
    }
}
