use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

fn signing_plan(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut plan = Vec::new();
    let mut entries = fs::read_dir(root)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_dir() {
            plan.extend(signing_plan(&path)?);
        } else if kind.is_file() {
            let mut magic = [0; 4];
            let header = fs::File::open(&path)?.read_exact(&mut magic);
            if let Err(error) = &header {
                if error.kind() != io::ErrorKind::UnexpectedEof {
                    return Err(io::Error::new(error.kind(), error.to_string()));
                }
            }
            if header.is_ok()
                && matches!(
                    u32::from_be_bytes(magic),
                    0xfeedface
                        | 0xcefaedfe
                        | 0xfeedfacf
                        | 0xcffaedfe
                        | 0xcafebabe
                        | 0xbebafeca
                        | 0xcafebabf
                        | 0xbfbafeca
                )
            {
                plan.push(path);
            }
        }
    }
    if matches!(
        root.extension().and_then(|value| value.to_str()),
        Some("app" | "framework" | "xpc")
    ) {
        plan.push(root.to_path_buf());
    }
    Ok(plan)
}

fn entitlements(existing: Option<plist::Value>, electron: bool) -> io::Result<plist::Dictionary> {
    let mut values = match existing {
        Some(plist::Value::Dictionary(values)) => values,
        Some(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "codesign entitlements must be a dictionary",
            ))
        }
        None => plist::Dictionary::new(),
    };
    if electron {
        values.insert("com.apple.security.cs.allow-jit".into(), true.into());
        values.insert(
            "com.apple.security.cs.disable-library-validation".into(),
            true.into(),
        );
    }
    Ok(values)
}

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::{env, process::Command};
    use tauri_macos_sign::Keychain;

    let keychain = match (
        env::var_os("APPLE_CERTIFICATE"),
        env::var_os("APPLE_CERTIFICATE_PASSWORD"),
    ) {
        (Some(certificate), Some(password)) => Keychain::with_certificate(&certificate, &password)?,
        (None, None) => match env::var("APPLE_SIGNING_IDENTITY") {
            Ok(identity) if identity != "-" && !identity.is_empty() => {
                Keychain::with_signing_identity(identity)
            }
            _ => {
                println!("No Developer ID configured; leaving bundled resources unsigned");
                return Ok(());
            }
        },
        _ => {
            return Err(
                "APPLE_CERTIFICATE and APPLE_CERTIFICATE_PASSWORD must be supplied together".into(),
            )
        }
    };
    if !keychain
        .signing_identity()
        .starts_with("Developer ID Application:")
    {
        return Err("bundled resources require a Developer ID Application identity".into());
    }
    let roots = env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if roots.is_empty() {
        return Err("expected Electron and resource directories".into());
    }
    let temp = tempfile::tempdir()?;
    for root in roots {
        let plan = signing_plan(&root)?;
        for path in plan {
            let output = Command::new("codesign")
                .args(["--display", "--entitlements", ":-"])
                .arg(&path)
                .output()?;
            if !output.status.success()
                && !String::from_utf8_lossy(&output.stderr)
                    .contains("code object is not signed at all")
            {
                return Err(format!(
                    "cannot read entitlements for {}: {}",
                    path.display(),
                    String::from_utf8_lossy(&output.stderr)
                )
                .into());
            }
            let existing = if output.stdout.is_empty() {
                None
            } else {
                Some(plist::Value::from_reader_xml(output.stdout.as_slice())?)
            };
            let electron = path
                .components()
                .any(|part| part.as_os_str().to_string_lossy().ends_with(".app"))
                && (path
                    .extension()
                    .is_some_and(|value| value == "app" || value == "xpc")
                    || path
                        .parent()
                        .is_some_and(|parent| parent.ends_with("Contents/MacOS")));
            let values = entitlements(existing, electron)?;
            let entitlement_path = temp.path().join("entitlements.plist");
            plist::Value::Dictionary(values.clone()).to_file_xml(&entitlement_path)?;
            keychain.sign(
                &path,
                (!values.is_empty()).then_some(entitlement_path.as_path()),
                true,
            )?;
            if !Command::new("codesign")
                .args(["--verify", "--strict"])
                .arg(&path)
                .status()?
                .success()
            {
                return Err(
                    format!("resource signature verification failed: {}", path.display()).into(),
                );
            }
        }
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("resource signing requires macOS");
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resources_are_signed_inside_out_including_extensionless_macho_and_addons() {
        let root = tempfile::tempdir().unwrap();
        let app = root
            .path()
            .join("runtime/darwin-arm64/DeepSeek Harness.app");
        let framework = app.join("Contents/Frameworks/Electron Framework.framework");
        let files = [
            framework.join("Versions/A/Helpers/chrome_crashpad_handler"),
            framework.join("Versions/A/Electron Framework"),
            app.join("Contents/Frameworks/DeepSeek Harness Helper (Renderer).app/Contents/MacOS/DeepSeek Harness Helper (Renderer)"),
            app.join("Contents/Frameworks/Squirrel.framework/Versions/A/Resources/ShipIt"),
            app.join("Contents/MacOS/DeepSeek Harness"),
            root.path().join("runtime/darwin-arm64/runtime/appkit.node"),
            root.path().join("resources/node_modules/@tailwindcss/oxide-darwin-arm64/tailwindcss-oxide.darwin-arm64.node"),
        ];
        for (index, path) in files.iter().enumerate() {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(
                path,
                if index == 1 {
                    &[0xca, 0xfe, 0xba, 0xbf][..]
                } else {
                    &[0xcf, 0xfa, 0xed, 0xfe][..]
                },
            )
            .unwrap();
        }
        fs::write(root.path().join("resources/text.node"), "not native").unwrap();
        fs::write(app.join("Contents/Info.plist"), "not code").unwrap();
        let plan = signing_plan(root.path()).unwrap();
        for file in &files {
            assert_eq!(
                plan.iter().filter(|path| *path == file).count(),
                1,
                "{}",
                file.display()
            );
        }
        for bundle in [&app, &framework] {
            let position = plan
                .iter()
                .position(|path| path == bundle)
                .expect("bundle seal missing");
            assert!(plan[..position].iter().any(|path| path.starts_with(bundle)));
            assert!(!plan[position + 1..]
                .iter()
                .any(|path| path.starts_with(bundle)));
        }
        assert!(!plan.contains(&root.path().join("resources/text.node")));
        assert!(!plan.contains(&app.join("Contents/Info.plist")));
    }

    #[test]
    fn electron_jit_and_native_addon_permissions_preserve_node_or_helper_entitlements() {
        let existing = plist::Value::from_reader_xml(br#"<?xml version="1.0"?><plist version="1.0"><dict><key>com.apple.security.cs.allow-unsigned-executable-memory</key><true/><key>com.apple.security.device.audio-input</key><true/></dict></plist>"#.as_slice()).unwrap();
        let preserved = entitlements(Some(existing.clone()), false).unwrap();
        assert_eq!(preserved.len(), 2);
        assert_eq!(
            preserved["com.apple.security.cs.allow-unsigned-executable-memory"].as_boolean(),
            Some(true)
        );
        let electron = entitlements(Some(existing), true).unwrap();
        assert_eq!(
            electron["com.apple.security.cs.allow-jit"].as_boolean(),
            Some(true)
        );
        assert_eq!(
            electron
                .get("com.apple.security.cs.disable-library-validation")
                .and_then(plist::Value::as_boolean),
            Some(true)
        );
        assert_eq!(
            electron["com.apple.security.device.audio-input"].as_boolean(),
            Some(true)
        );
        assert_eq!(
            electron["com.apple.security.cs.allow-unsigned-executable-memory"].as_boolean(),
            Some(true)
        );
        assert_eq!(
            entitlements(Some(plist::Value::String("invalid".into())), true)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[cfg(unix)]
    #[test]
    fn framework_aliases_and_external_symlinks_are_not_followed() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        fs::write(external.path().join("external"), [0xcf, 0xfa, 0xed, 0xfe]).unwrap();
        fs::write(root.path().join("native"), [0xcf, 0xfa, 0xed, 0xfe]).unwrap();
        symlink(root.path().join("native"), root.path().join("alias")).unwrap();
        symlink(external.path(), root.path().join("external")).unwrap();
        assert_eq!(
            signing_plan(root.path()).unwrap(),
            vec![root.path().join("native")]
        );
    }

    #[test]
    fn missing_resource_root_fails_instead_of_silently_skipping() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            signing_plan(&root.path().join("missing"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }
}
