use launcherlauncher::patch::{Action, Rule, java, pe};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "launcherlauncher-test-{}-{}",
        name,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn run(cmd: &mut Command, cwd: &Path) -> (bool, String, String) {
    let out = cmd.current_dir(cwd).output().expect("spawn command");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn rule(class: &str, method: &str, action: Action) -> Rule {
    Rule {
        id: String::new(),
        class: class.into(),
        method: method.into(),
        desc: None,
        action,
        note: String::new(),
    }
}

#[test]
fn java_class_gate_is_rewritten() {
    if Command::new("javac").arg("-version").output().is_err() {
        eprintln!("javac missing; skipping");
        return;
    }
    let dir = tmp("java");
    fs::write(
        dir.join("Gate.java"),
        r#"
public class Gate {
    static boolean downloadAllowed() { return false; }
    static boolean isOutdated() { return true; }
    static void checkUpdate() { System.out.println("checking"); }
    public static void main(String[] args) {
        System.out.println("download=" + downloadAllowed());
        System.out.println("outdated=" + isOutdated());
        checkUpdate();
    }
}
"#,
    )
    .unwrap();
    let (ok, out, err) = run(Command::new("javac").arg("Gate.java"), &dir);
    assert!(ok, "javac failed: {out}\n{err}");

    let original = fs::read(dir.join("Gate.class")).unwrap();
    let rules = vec![
        rule("Gate", "downloadAllowed", Action::ReturnTrue),
        rule("Gate", "isOutdated", Action::ReturnFalse),
        rule("Gate", "checkUpdate", Action::ReturnVoid),
    ];
    let mut report = Default::default();
    let patched = java::patch_class(&original, &rules, &mut report)
        .expect("patch")
        .expect("changed");
    assert_eq!(report.applied.len(), 3, "{report:?}");
    fs::write(dir.join("Gate.class"), patched).unwrap();

    let (ok, out, err) = run(Command::new("java").arg("Gate"), &dir);
    assert!(ok, "java failed: {err}");
    assert!(
        out.contains("download=True") || out.contains("download=true"),
        "{out}"
    );
    assert!(
        out.contains("outdated=False") || out.contains("outdated=false"),
        "{out}"
    );
    assert!(!out.contains("checking"), "{out}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn dotnet_methods_are_rewritten_in_place() {
    if Command::new("dotnet").arg("--version").output().is_err() {
        eprintln!("dotnet missing; skipping");
        return;
    }
    let dir = tmp("dotnet");
    fs::write(
        dir.join("fixture.csproj"),
        r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>net10.0</TargetFramework>
    <ImplicitUsings>enable</ImplicitUsings>
    <Nullable>enable</Nullable>
    <AssemblyName>fixture</AssemblyName>
  </PropertyGroup>
</Project>
"#,
    )
    .unwrap();
    fs::write(
        dir.join("Program.cs"),
        r#"
class Gate {
    static bool DownloadAllowed() => false;
    static bool IsOutdated() => true;
    static void CheckUpdate() => Console.WriteLine("checking");
    static void Main() {
        Console.WriteLine($"download={DownloadAllowed()}");
        Console.WriteLine($"outdated={IsOutdated()}");
        CheckUpdate();
    }
}
"#,
    )
    .unwrap();

    let (ok, out, err) = run(
        Command::new("dotnet").args(["build", "-c", "Release", "--nologo", "-v", "q"]),
        &dir,
    );
    assert!(ok, "dotnet build failed:\n{out}\n{err}");
    let dll = dir.join("bin/Release/net10.0/fixture.dll");
    let original = fs::read(&dll).unwrap();
    assert_eq!(
        pe::probe_assembly_version(&original).as_deref(),
        Some("1.0.0.0")
    );

    let rules = vec![
        rule("Gate", "DownloadAllowed", Action::ReturnTrue),
        rule("Gate", "IsOutdated", Action::ReturnFalse),
        rule("Gate", "CheckUpdate", Action::ReturnVoid),
    ];
    let (patched, report) = pe::patch_pe(&original, &rules).expect("patch");
    assert_eq!(patched.len(), original.len());
    assert_eq!(report.applied.len(), 3, "{report:?}");
    fs::write(&dll, patched).unwrap();

    let (ok, out, err) = run(Command::new("dotnet").arg(&dll), &dir);
    assert!(ok, "dotnet run failed:\n{out}\n{err}");
    assert!(
        out.contains("download=True") || out.contains("download=true"),
        "{out}"
    );
    assert!(
        out.contains("outdated=False") || out.contains("outdated=false"),
        "{out}"
    );
    assert!(!out.contains("checking"), "{out}");
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(target_os = "linux")]
#[test]
fn dotnet_single_file_bundle_is_patched() {
    if Command::new("dotnet").arg("--version").output().is_err() {
        eprintln!("dotnet missing; skipping");
        return;
    }
    let dir = tmp("dotnet-single");
    fs::write(
        dir.join("fixture.csproj"),
        r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>net10.0</TargetFramework>
    <ImplicitUsings>enable</ImplicitUsings>
    <Nullable>enable</Nullable>
    <AssemblyName>fixture</AssemblyName>
    <RuntimeIdentifier>linux-x64</RuntimeIdentifier>
    <PublishSingleFile>true</PublishSingleFile>
    <SelfContained>false</SelfContained>
  </PropertyGroup>
</Project>
"#,
    )
    .unwrap();
    fs::write(
        dir.join("Program.cs"),
        r#"
class Gate {
    static bool DownloadAllowed() => false;
    public static void Main() {
        Console.WriteLine($"download={DownloadAllowed()}");
    }
}
"#,
    )
    .unwrap();

    let (ok, out, err) = run(
        Command::new("dotnet").args([
            "publish", "-c", "Release", "--nologo", "-v", "q", "-o", "publish",
        ]),
        &dir,
    );
    assert!(ok, "dotnet publish failed:\n{out}\n{err}");
    let exe = dir.join("publish/fixture");
    let original = fs::read(&exe).unwrap();
    let rules = vec![rule("Gate", "DownloadAllowed", Action::ReturnTrue)];
    let (patched, report) = pe::patch_pe(&original, &rules).expect("patch");
    assert_eq!(patched.len(), original.len());
    assert_eq!(report.applied.len(), 1, "{report:?}");
    fs::write(&exe, patched).unwrap();

    let (ok, out, err) = run(&mut Command::new(&exe), &dir);
    assert!(ok, "single-file run failed:\n{out}\n{err}");
    assert!(
        out.contains("download=True") || out.contains("download=true"),
        "{out}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn java_branch_and_exception_handler_are_rewritten() {
    if Command::new("javac").arg("-version").output().is_err() {
        eprintln!("javac missing; skipping");
        return;
    }
    let dir = tmp("java-branch");
    fs::write(
        dir.join("Gate2.java"),
        r#"
public class Gate2 {
    static boolean allowed(int x) {
        if (x > 0) return false;
        try {
            System.out.println("try");
        } catch (RuntimeException e) {
            System.out.println("catch");
        }
        return true;
    }
    public static void main(String[] args) {
        System.out.println("allowed=" + allowed(1));
    }
}
"#,
    )
    .unwrap();
    let (ok, out, err) = run(Command::new("javac").arg("Gate2.java"), &dir);
    assert!(ok, "javac failed:\n{out}\n{err}");

    let original = fs::read(dir.join("Gate2.class")).unwrap();
    let rules = vec![rule("Gate2", "allowed", Action::ReturnFalse)];
    let mut report = Default::default();
    let patched = java::patch_class(&original, &rules, &mut report)
        .expect("patch")
        .expect("changed");
    assert_eq!(report.applied.len(), 1, "{report:?}");
    fs::write(dir.join("Gate2.class"), patched).unwrap();

    let (ok, out, err) = run(Command::new("java").arg("Gate2"), &dir);
    assert!(ok, "java failed: {err}");
    assert!(out.contains("allowed=false"), "{out}");
    let _ = fs::remove_dir_all(&dir);
}
