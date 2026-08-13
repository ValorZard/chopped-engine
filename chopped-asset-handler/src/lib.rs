use thiserror::Error;

#[derive(Error, Debug)]
pub enum AssetFetchError {
    #[error("failed to request asset '{path}'")]
    PathFailure { path: String },
    #[error("asset '{path}' returned HTTP {status}")]
    HTTPError { path: String, status: u16 },
    #[error("failed to read asset '{path}' body")]
    BinaryReadFailure { path: String },
}

#[cfg(target_arch = "wasm32")]
pub async fn fetch_asset_bytes_wasm(relative_path: &str) -> Result<Vec<u8>, AssetFetchError> {
    use gloo_net::http::Request;

    let path = format!("assets/{relative_path}");
    let Ok(response) = Request::get(&path).send().await else {
        return Err(AssetFetchError::PathFailure { path });
    };

    if !response.ok() {
        return Err(AssetFetchError::HTTPError {
            path,
            status: response.status(),
        });
    }

    match response.binary().await {
        Ok(vec) => {
            return Ok(vec);
        }
        Err(_) => return Err(AssetFetchError::BinaryReadFailure { path }),
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn fetch_asset_bytes_native(
    relative_path: &str,
    manifest_dir: &str,
) -> Result<Vec<u8>, AssetFetchError> {
    let dev_path = std::path::Path::new(manifest_dir)
        .join("assets")
        .join(relative_path);
    match std::fs::read(&dev_path) {
        Ok(vec) => {
            return Ok(vec);
        }
        Err(_) => {
            return Err(AssetFetchError::PathFailure {
                path: dev_path.to_str().unwrap_or_default().to_string(),
            });
        }
    }
}

/// Fetches an asset's raw bytes given a path relative to the `assets/` folder,
/// working the same way on both the wasm32 (browser) and native builds.
///
/// - On wasm32, this issues an HTTP GET relative to the page URL,
///     so it relies on `Trunk.toml`'s `public_url = "."`
///     and the assets actually being copied into `dist/`
///     (see the `copy-dir` link in `index.html`)
///     this is what lets it keep working once itch.io serves the game from a hashed subpath.
/// - On native, this reads from disk next to the executable,
///     since `upload_client.sh` ships an `assets/` folder alongside the binary.
///     It falls back to `CARGO_MANIFEST_DIR` so `cargo run` works without staging
///     assets next to `target/debug/client.exe`.
pub async fn fetch_asset_bytes(
    relative_path: &str,
    manifest_dir: &str,
) -> Result<Vec<u8>, AssetFetchError> {
    cfg_select! {
        target_arch = "wasm32" => {
            fetch_asset_bytes_wasm(relative_path).await
        }
        _ => {
            fetch_asset_bytes_native(relative_path, manifest_dir)
        }
    }
}
