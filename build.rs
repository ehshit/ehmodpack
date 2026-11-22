use std::env;
use std::fs;
use std::path::PathBuf;

fn write_catalogs() {
    let root = PathBuf::from("locales");
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR must exist")).join("catalogs.rs");
    let mut locales = Vec::new();
    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let file = path.join("cli.ftl");
            if file.is_file() {
                let name = entry.file_name().to_string_lossy().into_owned();
                println!("cargo:rerun-if-changed={}", file.display());
                locales.push((name, file));
            }
        }
    }
    println!("cargo:rerun-if-changed={}", root.display());
    locales.sort();
    let entries: Vec<String> = locales
        .into_iter()
        .map(|(name, path)| {
            let path = path.to_string_lossy().replace('\\', "/");
            format!(
                "({:?}, include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/{}\"))),",
                name, path
            )
        })
        .collect();
    fs::write(out, format!("&[\n    {}\n]\n", entries.join("\n    ")))
        .expect("could not write the catalog index");
}

fn bundled_client_id() -> Option<String> {
    if let Ok(value) = env::var("EHMODPACK_MSA_CLIENT_ID") {
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    let text = std::fs::read_to_string(".env").ok()?;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "EHMODPACK_MSA_CLIENT_ID" {
            continue;
        }
        let value = value.trim().trim_matches(['"', '\'']).trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

fn main() {
    write_catalogs();
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=.env");
    println!("cargo:rerun-if-env-changed=EHMODPACK_MSA_CLIENT_ID");
    println!(
        "cargo:rustc-env=EHMODPACK_MSA_CLIENT_ID_BUNDLED={}",
        bundled_client_id().unwrap_or_default()
    );

    #[cfg(target_os = "windows")]
    {
        let icon = ".github/icon/icon-windows.ico";
        println!("cargo:rerun-if-changed={icon}");
        if let Err(why) = winres::WindowsResource::new()
            .set_icon(icon)
            .set("FileDescription", "ehmodpack")
            .set("ProductName", "ehmodpack")
            .compile()
        {
            println!("cargo:warning=could not embed the windows icon: {why}");
        }
    }
}
