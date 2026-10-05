mod scenes;
mod svg;

use std::{env, fs, path::Path};

fn snapshots(output: &Path, check: bool) -> Result<Vec<&'static str>, String> {
    // Render and export every scene before touching output or reporting any success.
    let images = scenes::SCENES
        .iter()
        .map(|&(name, scene, width, height)| {
            let buffer =
                scenes::render(scene, width, height).map_err(|error| format!("{name}: {error}"))?;
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
    let check = match args.as_slice() {
        [command] if command == "ui-snapshots" => false,
        [command, flag] if command == "ui-snapshots" && flag == "--check" => true,
        _ => return Err("Usage: cargo xtask ui-snapshots [--check]".into()),
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("Cannot locate repository root")?;
    let names = snapshots(&root.join("artifacts/ui"), check)?;
    println!(
        "{} {} UI snapshots:",
        if check { "Verified" } else { "Generated" },
        names.len()
    );
    for name in names {
        println!("  artifacts/ui/{name}");
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
    fn generation_creates_directory_overwrites_files_and_propagates_io_errors() {
        let root = env::temp_dir().join(format!("mypowers-ui-snapshots-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let output = root.join("ui");
        let names = snapshots(&output, false).unwrap();
        let path = output.join(names[0]);
        let expected = fs::read(&path).unwrap();
        fs::write(&path, "obsolete").unwrap();
        assert_eq!(snapshots(&output, false).unwrap(), names);
        assert_eq!(fs::read(path).unwrap(), expected);
        let blocked = root.join("file");
        fs::write(&blocked, "not a directory").unwrap();
        assert!(snapshots(&blocked, false).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn check_detects_changed_and_missing_snapshots_without_writing_any_files() {
        let root = env::temp_dir().join(format!("mypowers-ui-check-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        assert!(snapshots(&root, true).is_err());
        assert!(
            !root.exists(),
            "checking must not create the output directory"
        );
        let names = snapshots(&root, false).unwrap();
        assert_eq!(snapshots(&root, true).unwrap(), names);
        let changed = root.join(names[0]);
        let missing = root.join(names[1]);
        let untouched = root.join(names[2]);
        let expected = fs::read(&untouched).unwrap();
        fs::write(&changed, "changed snapshot").unwrap();
        fs::remove_file(&missing).unwrap();
        let error = snapshots(&root, true).unwrap_err();
        assert!(error.contains(names[0]) && error.contains(names[1]));
        assert_eq!(fs::read(&changed).unwrap(), b"changed snapshot");
        assert!(!missing.exists());
        assert_eq!(fs::read(&untouched).unwrap(), expected);
        fs::remove_dir_all(root).unwrap();
    }
}
