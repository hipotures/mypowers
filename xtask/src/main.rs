mod scenes;
mod svg;

use std::{env, fs, path::Path};

fn snapshots(output: &Path, check: bool, gallery: bool) -> Result<Vec<&'static str>, String> {
    snapshots_with_data(output, check, gallery, None)
}

fn snapshots_with_data(
    output: &Path,
    check: bool,
    gallery: bool,
    data: Option<&serde_json::Value>,
) -> Result<Vec<&'static str>, String> {
    // Render and export every scene before touching output or reporting any success.
    let images = scenes::SCENES
        .iter()
        .filter(|&&(_, _, width, height)| !gallery || (width == 120 && height == 30))
        .chain(if gallery {
            scenes::GALLERY.iter()
        } else {
            [].iter()
        })
        .filter(|&&(_, scene, _, _)| data.is_none() || scenes::live_gallery_scene(scene))
        .map(|&(name, scene, width, height)| {
            let (width, height) = if gallery { (98, 31) } else { (width, height) };
            let buffer = match data {
                Some(data) => scenes::render_captured(scene, width, height, data),
                None => scenes::render(scene, width, height),
            }
            .map_err(|error| format!("{name}: {error}"))?;
            let svg = svg::export(&buffer).map_err(|error| format!("{name}: {error}"))?;
            Ok((name, svg))
        })
        .collect::<Result<Vec<_>, String>>()?;
    if check {
        let mut failures = Vec::new();
        for (name, svg) in &images {
            match fs::read(output.join(name)) {
                Ok(expected) if expected == svg.as_bytes() => {}
                Ok(_) => failures.push(format!("{name}: differs from the saved snapshot")),
                Err(error) => failures.push(format!("{name}: {error}")),
            }
        }
        if !failures.is_empty() {
            return Err(failures.join("\n"));
        }
    } else {
        fs::create_dir_all(output).map_err(|error| format!("{}: {error}", output.display()))?;
        for (name, svg) in &images {
            fs::write(output.join(name), svg).map_err(|error| format!("{name}: {error}"))?;
        }
    }
    Ok(images.into_iter().map(|(name, _)| name).collect())
}

fn run() -> Result<(), String> {
    let args: Vec<_> = env::args().skip(1).collect();
    let usage =
        "Usage: cargo xtask ui-snapshots [--check] [--gallery] [--output PATH] [--data PATH]";
    if args.first().map(String::as_str) != Some("ui-snapshots") {
        return Err(usage.into());
    }
    let mut check = false;
    let mut gallery = false;
    let mut selected_output = None;
    let mut selected_data = None;
    let mut remaining = args.iter().skip(1);
    while let Some(flag) = remaining.next() {
        match flag.as_str() {
            "--check" => check = true,
            "--gallery" => gallery = true,
            "--data" => selected_data = Some(remaining.next().ok_or(usage)?),
            "--output" => selected_output = Some(remaining.next().ok_or(usage)?),
            _ => return Err(usage.into()),
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("Cannot locate repository root")?;
    if gallery && selected_output.is_none() {
        return Err("Gallery requires --output PATH to preserve canonical snapshots.".into());
    }
    let output = selected_output
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join("artifacts/ui"));
    if selected_data.is_some() && !gallery {
        return Err("Captured data requires --gallery --output PATH.".into());
    }
    let data: Option<serde_json::Value> = selected_data
        .map(|path| {
            let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
            serde_json::from_str(&content).map_err(|e| e.to_string())
        })
        .transpose()?;
    let names = if data.is_some() {
        snapshots_with_data(&output, check, gallery, data.as_ref())?
    } else {
        snapshots(&output, check, gallery)?
    };
    if gallery && !check {
        let manifest = serde_json::json!({"width":980,"height":620,"source":if data.is_some(){"daemon telemetry and database history"}else{"synthetic fixtures"},"captured_at":data.as_ref().map(|d| &d["status"]["server_time"]),"screenshots":names});
        fs::write(
            output.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    println!(
        "{} {} UI snapshots:",
        if check { "Verified" } else { "Generated" },
        names.len()
    );
    for name in names {
        println!("  {}", output.join(name).display());
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("UI snapshot task failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gallery_has_unique_names_uniform_size_and_no_resize_variants() {
        let output = env::temp_dir().join(format!("mypowers-ui-gallery-{}", std::process::id()));
        let names = snapshots(&output, false, true).unwrap();
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(names.len(), unique.len());
        assert!(names.contains(&"settings-alert-threshold-picker.svg"));
        assert!(names.contains(&"settings-notify-sent.svg"));
        for name in names {
            let image = fs::read_to_string(output.join(name)).unwrap();
            assert!(
                image.starts_with(
                    "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"980\" height=\"620\""
                ),
                "{name}"
            );
            assert!(!name.contains("80x24") && !name.contains("60x19"));
        }
        snapshots(&output, true, true).unwrap();
        fs::remove_dir_all(output).unwrap();
    }

    #[test]
    fn generation_creates_directory_overwrites_files_and_propagates_io_errors() {
        let root = env::temp_dir().join(format!("mypowers-ui-snapshots-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let output = root.join("ui");
        let names = snapshots(&output, false, false).unwrap();
        let path = output.join(names[0]);
        let expected = fs::read(&path).unwrap();
        fs::write(&path, "obsolete").unwrap();
        assert_eq!(snapshots(&output, false, false).unwrap(), names);
        assert_eq!(fs::read(path).unwrap(), expected);
        let blocked = root.join("file");
        fs::write(&blocked, "not a directory").unwrap();
        assert!(snapshots(&blocked, false, false).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn check_detects_changed_and_missing_snapshots_without_writing_any_files() {
        let root = env::temp_dir().join(format!("mypowers-ui-check-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        assert!(snapshots(&root, true, false).is_err());
        assert!(
            !root.exists(),
            "checking must not create the output directory"
        );
        let names = snapshots(&root, false, false).unwrap();
        assert_eq!(snapshots(&root, true, false).unwrap(), names);
        let changed = root.join(names[0]);
        let missing = root.join(names[1]);
        let untouched = root.join(names[2]);
        let expected = fs::read(&untouched).unwrap();
        fs::write(&changed, "changed snapshot").unwrap();
        fs::remove_file(&missing).unwrap();
        let error = snapshots(&root, true, false).unwrap_err();
        assert!(error.contains(names[0]) && error.contains(names[1]));
        assert_eq!(fs::read(&changed).unwrap(), b"changed snapshot");
        assert!(!missing.exists());
        assert_eq!(fs::read(&untouched).unwrap(), expected);
        fs::remove_dir_all(root).unwrap();
    }
}
