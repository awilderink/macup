use anyhow::{Result, anyhow, bail};
use block2::RcBlock;
use objc2_foundation::{NSArray, NSError, NSString, NSURL};
use objc2_photos::{
    PHAccessLevel, PHAsset, PHAssetResource, PHAssetResourceManager, PHAssetResourceRequestOptions,
    PHAssetResourceType, PHAuthorizationStatus, PHPhotoLibrary,
};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
pub enum Access {
    Granted,
    NotAsked,
    Denied,
}

pub fn access() -> Access {
    match unsafe { PHPhotoLibrary::authorizationStatusForAccessLevel(PHAccessLevel::ReadWrite) } {
        PHAuthorizationStatus::Authorized | PHAuthorizationStatus::Limited => Access::Granted,
        PHAuthorizationStatus::NotDetermined => Access::NotAsked,
        _ => Access::Denied,
    }
}

pub fn ensure_access() -> Result<()> {
    if access() == Access::NotAsked {
        crate::ui::note("macOS will ask once to give your terminal access to Photos");
        request_access()?;
    }
    if access() != Access::Granted {
        bail!(
            "no access to Photos; allow it in System Settings → Privacy & Security → Photos, then try again"
        );
    }
    Ok(())
}

fn request_access() -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let handler = RcBlock::new(move |status: PHAuthorizationStatus| {
        let _ = tx.send(status);
    });
    unsafe {
        PHPhotoLibrary::requestAuthorizationForAccessLevel_handler(
            PHAccessLevel::ReadWrite,
            &handler,
        )
    };
    rx.recv_timeout(Duration::from_secs(600))
        .map_err(|_| anyhow!("no answer to the Photos access request"))?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Primary,
    PairedVideo,
    Alternate,
    Audio,
}

pub struct Downloaded {
    pub kind: Kind,
    pub original_name: String,
    pub path: PathBuf,
}

fn kind_of(t: PHAssetResourceType) -> Option<Kind> {
    match t {
        PHAssetResourceType::Photo | PHAssetResourceType::Video => Some(Kind::Primary),
        PHAssetResourceType::PairedVideo => Some(Kind::PairedVideo),
        PHAssetResourceType::AlternatePhoto => Some(Kind::Alternate),
        PHAssetResourceType::Audio => Some(Kind::Audio),
        _ => None,
    }
}

pub fn download_originals(uuid: &str, dir: &Path) -> Result<Vec<Downloaded>> {
    let ids = NSArray::from_retained_slice(&[NSString::from_str(&format!("{uuid}/L0/001"))]);
    let assets = unsafe { PHAsset::fetchAssetsWithLocalIdentifiers_options(&ids, None) };
    let Some(asset) = (unsafe { assets.firstObject() }) else {
        bail!("Photos does not know this item")
    };
    let resources = unsafe { PHAssetResource::assetResourcesForAsset(&asset) };
    let manager = unsafe { PHAssetResourceManager::defaultManager() };
    let options = unsafe { PHAssetResourceRequestOptions::new() };
    unsafe { options.setNetworkAccessAllowed(true) };
    let mut out = Vec::new();
    for (n, resource) in resources.iter().enumerate() {
        let Some(kind) = kind_of(unsafe { resource.r#type() }) else {
            continue;
        };
        let original_name = unsafe { resource.originalFilename() }.to_string();
        let path = dir.join(format!(".incoming-{uuid}-{n}"));
        let _ = std::fs::remove_file(&path);
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.display().to_string()));
        let (tx, rx) = mpsc::channel::<Option<String>>();
        let done = RcBlock::new(move |err: *mut NSError| {
            let message = unsafe { err.as_ref() }.map(|e| e.localizedDescription().to_string());
            let _ = tx.send(message);
        });
        unsafe {
            manager.writeDataForAssetResource_toFile_options_completionHandler(
                &resource,
                &url,
                Some(&options),
                &done,
            )
        };
        match rx.recv_timeout(Duration::from_secs(1800)) {
            Ok(None) => out.push(Downloaded {
                kind,
                original_name,
                path,
            }),
            Ok(Some(message)) => bail!("downloading {original_name}: {message}"),
            Err(_) => bail!("downloading {original_name} timed out"),
        }
    }
    if out.is_empty() {
        bail!("Photos returned no original files for this item");
    }
    Ok(out)
}
