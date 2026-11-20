use std::path::PathBuf;

fn find_rc() -> Option<PathBuf> {
    let base = std::env::var_os("ProgramFiles(x86)")?;
    let root = PathBuf::from(base)
        .join("Windows Kits")
        .join("10")
        .join("bin");

    let mut best: Option<(Vec<u32>, PathBuf)> = None;
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let candidate = entry.path().join("x64").join("rc.exe");
        if !candidate.exists() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let key: Vec<u32> = name
            .split('.')
            .map(|part| part.parse::<u32>().unwrap_or(0))
            .collect();
        if best
            .as_ref()
            .map(|(current, _)| key > *current)
            .unwrap_or(true)
        {
            best = Some((key, candidate));
        }
    }
    best.map(|(_, path)| path)
}

fn main() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=assets/quickphrase.ico");

    // Neither rc.exe nor llvm-rc is on PATH on a typical build machine.
    if std::env::var_os("RC").is_none() {
        if let Some(rc) = find_rc() {
            println!("cargo:warning=using resource compiler {}", rc.display());
            std::env::set_var("RC", rc);
        }
    }

    let result = embed_resource::compile("app.rc", embed_resource::NONE);
    println!("cargo:warning=embed-resource: {result}");
    if let Err(err) = result.manifest_required() {
        panic!("resource compilation failed: {err}");
    }
}
