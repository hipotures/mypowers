mod scenes;
mod svg;

use std::{env, fs, path::Path};

fn generate(output: &Path) -> Result<Vec<&'static str>, String> {
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
    fs::create_dir_all(output).map_err(|error| format!("{}: {error}", output.display()))?;
    for (name, svg) in &images {
        fs::write(output.join(name), svg).map_err(|error| format!("{name}: {error}"))?;
    }
    Ok(images.into_iter().map(|(name, _)| name).collect())
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    if args.next().as_deref() != Some("ui-snapshots") || args.next().is_some() {
        return Err("Usage: cargo xtask ui-snapshots".into());
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("Cannot locate repository root")?;
    let names = generate(&root.join("artifacts/ui"))?;
    println!("Generated {} UI snapshots:", names.len());
    for name in names {
        println!("  artifacts/ui/{name}");
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("UI snapshot generation failed: {error}");
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
        let names = generate(&output).unwrap();
        let path = output.join(names[0]);
        let expected = fs::read(&path).unwrap();
        fs::write(&path, "obsolete").unwrap();
        assert_eq!(generate(&output).unwrap(), names);
        assert_eq!(fs::read(path).unwrap(), expected);
        let blocked = root.join("file");
        fs::write(&blocked, "not a directory").unwrap();
        assert!(generate(&blocked).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
