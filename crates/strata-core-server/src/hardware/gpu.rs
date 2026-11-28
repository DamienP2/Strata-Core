//! Les GPU du serveur (Réglages › Matériel › GPU) : NVIDIA, Intel, AMD. Pour chacun, son pilote, son
//! nœud de rendu (`/dev/dri/renderD128`), ce qu'il sait encoder et décoder (NVENC/NVDEC chez NVIDIA,
//! VA-API chez Intel et AMD), et sa charge. Les applis le lisent dans `/api/hardware/shared` pour
//! choisir où encoder, décoder ou rendre.
//!
//! Lu sur le bus PCI (`/sys`), avec `lspci` pour le nom, `nvidia-smi` et `vainfo` pour le reste ;
//! gardé 3 s. `STRATA_FAKE_GPU=nvidia,intel` : des GPU simulés.
use serde::Serialize;
use std::{
    path::Path,
    process::{Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// % du GPU.
    pub gpu: f32,
    /// % de l'encodeur et du décodeur matériels.
    pub encoder: f32,
    pub decoder: f32,
    pub memory_used_mb: u64,
    pub temperature: Option<f32>,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Gpu {
    /// L'adresse PCI (`0000:3b:00.0`).
    pub pci: String,
    /// `nvidia`, `intel`, `amd`, `other`.
    pub vendor: &'static str,
    pub name: String,
    /// Le pilote du noyau en place (`nvidia`, `nouveau`, `i915`, `xe`, `amdgpu`).
    pub driver: Option<String>,
    pub driver_version: Option<String>,
    /// Le nœud de rendu, pour les applis.
    pub render_node: Option<String>,
    pub memory_mb: Option<u64>,
    /// L'interface d'encodage et de décodage : `nvenc` (NVIDIA) ou `vaapi`.
    pub api: Option<&'static str>,
    /// Les codecs encodés et décodés par le matériel (`h264`, `hevc`, `av1`…).
    pub encode: Vec<String>,
    pub decode: Vec<String>,
    /// Prêt pour les applis : le bon pilote, et l'encodage en état.
    pub ready: bool,
    /// Ce qui manque, en clair.
    pub note: Option<String>,
    pub usage: Option<Usage>,
}

fn vendor_of(id: &str) -> &'static str {
    match id.trim() {
        "0x10de" => "nvidia",
        "0x8086" => "intel",
        "0x1002" => "amd",
        _ => "other",
    }
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// Ce qu'un outil écrit, en 5 s au plus : un pilote bloqué fait parfois attendre nvidia-smi pour
/// toujours, et il ne doit pas bloquer le Core (l'outil est alors arrêté).
fn output(prog: &str, args: &[&str]) -> Option<String> {
    use std::io::Read;
    let mut child = Command::new(prog).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = stdout.read_to_end(&mut text);
        text
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                tracing::warn!("{prog} did not answer in 5 s: stopped");
                break None;
            }
        }
    };
    let text = reader.join().unwrap_or_default();
    status?.success().then(|| String::from_utf8_lossy(&text).into_owned())
}

/// `lspci -mm` : `00:02.0 "VGA compatible controller" "Intel Corporation" "Device 9a49" …`.
fn lspci_name(slot: &str) -> Option<String> {
    let out = output("lspci", &["-mm", "-s", slot])?;
    let fields: Vec<&str> = out.split('"').filter(|f| !f.trim().is_empty()).collect();
    // slot, classe, fabricant, appareil…
    let device = fields.get(3)?.trim();
    let vendor = fields.get(2)?.trim();
    let short = vendor.split_whitespace().next().unwrap_or_default();
    Some(if device.starts_with("Device ") { format!("{vendor} {device}") } else { format!("{short} {device}") })
}

/// Les codecs d'une sortie de `vainfo` : (encodés, décodés).
pub fn vaapi_codecs(vainfo: &str) -> (Vec<String>, Vec<String>) {
    let mut enc: Vec<String> = Vec::new();
    let mut dec: Vec<String> = Vec::new();
    for l in vainfo.lines() {
        let Some((profile, entry)) = l.split_once(':') else { continue };
        let p = profile.trim();
        let codec = if p.starts_with("VAProfileH264") {
            "h264"
        } else if p.starts_with("VAProfileHEVC") {
            "hevc"
        } else if p.starts_with("VAProfileAV1") {
            "av1"
        } else if p.starts_with("VAProfileVP9") {
            "vp9"
        } else if p.starts_with("VAProfileJPEG") {
            "jpeg"
        } else {
            continue;
        };
        let e = entry.trim();
        let list = if e.starts_with("VAEntrypointEnc") {
            &mut enc
        } else if e == "VAEntrypointVLD" {
            &mut dec
        } else {
            continue;
        };
        if !list.iter().any(|c| c == codec) {
            list.push(codec.into());
        }
    }
    (enc, dec)
}

/// Ce que NVENC/NVDEC savent faire, d'après le modèle (AV1 à l'encodage depuis Ada, au décodage
/// depuis Ampere).
pub fn nvidia_codecs(name: &str) -> (Vec<String>, Vec<String>) {
    let n = name.to_uppercase();
    let ada = ["GEFORCE RTX 40", "GEFORCE RTX 50", " L4", " L40", "ADA", "BLACKWELL", "RTX PRO"].iter().any(|k| n.contains(k));
    let ampere = ada || ["GEFORCE RTX 30", " A2", " A10", " A16", " A40", "RTX A"].iter().any(|k| n.contains(k));
    let mut enc = vec!["h264".to_string(), "hevc".to_string()];
    let mut dec = vec!["h264".to_string(), "hevc".to_string(), "vp9".to_string()];
    if ada {
        enc.push("av1".into());
    }
    if ampere {
        dec.push("av1".into());
    }
    (enc, dec)
}

/// `nvidia-smi --query-gpu=pci.bus_id,name,driver_version,memory.total,memory.used,utilization.gpu,
/// utilization.encoder,utilization.decoder,temperature.gpu --format=csv,noheader,nounits`, par bus.
fn nvidia_smi() -> Vec<Vec<String>> {
    output(
        "nvidia-smi",
        &[
            "--query-gpu=pci.bus_id,name,driver_version,memory.total,memory.used,utilization.gpu,utilization.encoder,utilization.decoder,temperature.gpu",
            "--format=csv,noheader,nounits",
        ],
    )
    .map(|o| o.lines().map(|l| l.split(',').map(|f| f.trim().to_string()).collect()).collect())
    .unwrap_or_default()
}

/// `00000000:3B:00.0` et `0000:3b:00.0` : le même bus.
fn same_bus(a: &str, b: &str) -> bool {
    let tail = |s: &str| s.to_lowercase().rsplit(':').take(2).collect::<Vec<_>>().join(":");
    tail(a) == tail(b)
}

fn num(s: Option<&String>) -> Option<f64> {
    s.and_then(|v| v.parse::<f64>().ok())
}

fn scan() -> Vec<Gpu> {
    let mut out = Vec::new();
    let smi = nvidia_smi();
    for e in std::fs::read_dir("/sys/bus/pci/devices").into_iter().flatten().flatten() {
        let d = e.path();
        if !read(&d.join("class")).starts_with("0x03") {
            continue;
        }
        let pci = e.file_name().to_string_lossy().into_owned();
        let vendor = vendor_of(&read(&d.join("vendor")));
        // Seulement NVIDIA, Intel et AMD : l'écran virtuel d'une VM (QEMU, VMware, VirtualBox,
        // Hyper-V, virtio) ou celui du contrôleur d'un serveur (ASPEED, Matrox) n'encode rien.
        if vendor == "other" {
            continue;
        }
        let driver = std::fs::read_link(d.join("driver")).ok().and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()));
        let render_node = std::fs::read_dir(d.join("drm"))
            .ok()
            .and_then(|r| r.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).find(|n| n.starts_with("renderD")))
            .map(|n| format!("/dev/dri/{n}"));
        let mut g = Gpu {
            name: memo(format!("lspci {pci}"), || lspci_name(&pci)).unwrap_or_else(|| format!("{vendor} {}", read(&d.join("device")))),
            pci,
            vendor,
            driver: driver.clone(),
            render_node,
            ..Gpu::default()
        };
        match vendor {
            "nvidia" if driver.as_deref() == Some("nvidia") => {
                if let Some(row) = smi.iter().find(|r| r.first().is_some_and(|b| same_bus(b, &g.pci))) {
                    g.name = row.get(1).cloned().unwrap_or(g.name);
                    g.driver_version = row.get(2).cloned();
                    g.memory_mb = num(row.get(3)).map(|v| v as u64);
                    g.usage = Some(Usage {
                        memory_used_mb: num(row.get(4)).unwrap_or(0.0) as u64,
                        gpu: num(row.get(5)).unwrap_or(0.0) as f32,
                        encoder: num(row.get(6)).unwrap_or(0.0) as f32,
                        decoder: num(row.get(7)).unwrap_or(0.0) as f32,
                        temperature: num(row.get(8)).map(|v| v as f32),
                    });
                }
                let (enc, dec) = nvidia_codecs(&g.name);
                g.api = Some("nvenc");
                g.encode = enc;
                g.decode = dec;
                g.ready = true;
            }
            "nvidia" => {
                g.note = Some(match driver.as_deref() {
                    Some("nouveau") => "the open nouveau driver cannot encode: install the NVIDIA driver".into(),
                    _ => "no driver: install the NVIDIA driver".into(),
                });
            }
            "intel" | "amd" => {
                let node = g.render_node.clone().unwrap_or_else(|| "/dev/dri/renderD128".into());
                let vainfo = memo(format!("vainfo {node}"), || output("vainfo", &["--display", "drm", "--device", &node]));
                match (&g.render_node, vainfo) {
                    (None, _) => g.note = Some("no render node: driver missing".into()),
                    (Some(_), None) => g.note = Some("VA-API not ready: install vainfo and the VA-API driver".into()),
                    (Some(_), Some(v)) => {
                        let (enc, dec) = vaapi_codecs(&v);
                        g.driver_version = v.lines().find_map(|l| l.split_once("Driver version:").map(|(_, r)| r.trim().to_string()));
                        g.api = Some("vaapi");
                        g.ready = !enc.is_empty() || !dec.is_empty();
                        g.encode = enc;
                        g.decode = dec;
                    }
                }
            }
            _ => {}
        }
        out.push(g);
    }
    out.sort_by(|a, b| a.pci.cmp(&b.pci));
    out
}

fn wobble(scale: f32, seed: u32) -> f32 {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_millis()) + seed * 137;
    (n % 1000) as f32 / 1000.0 * scale
}

/// Des GPU simulés (`STRATA_FAKE_GPU=nvidia,intel,amd`).
fn fake(list: &str) -> Vec<Gpu> {
    list.split(',')
        .map(str::trim)
        .enumerate()
        .filter_map(|(i, kind)| {
            let pci = format!("0000:{:02x}:00.0", 0x65 + i * 0x10);
            let render = Some(format!("/dev/dri/renderD{}", 128 + i));
            Some(match kind {
                "nvidia" => {
                    let (enc, dec) = nvidia_codecs("NVIDIA RTX 4000 Ada Generation");
                    Gpu {
                        pci,
                        vendor: "nvidia",
                        name: "NVIDIA RTX 4000 Ada Generation".into(),
                        driver: Some("nvidia".into()),
                        driver_version: Some("550.120".into()),
                        render_node: render,
                        memory_mb: Some(20475),
                        api: Some("nvenc"),
                        encode: enc,
                        decode: dec,
                        ready: true,
                        note: None,
                        usage: Some(Usage {
                            gpu: 18.0 + wobble(8.0, 1),
                            encoder: 34.0 + wobble(6.0, 2),
                            decoder: 12.0 + wobble(4.0, 3),
                            memory_used_mb: 3120,
                            temperature: Some(54.0 + wobble(2.0, 4)),
                        }),
                    }
                }
                "nouveau" => Gpu {
                    pci,
                    vendor: "nvidia",
                    name: "NVIDIA RTX A2000".into(),
                    driver: Some("nouveau".into()),
                    render_node: render,
                    note: Some("the open nouveau driver cannot encode: install the NVIDIA driver".into()),
                    ..Gpu::default()
                },
                "intel" => Gpu {
                    pci,
                    vendor: "intel",
                    name: "Intel UHD Graphics 770".into(),
                    driver: Some("i915".into()),
                    driver_version: Some("Intel iHD driver 24.1.0".into()),
                    render_node: render,
                    api: Some("vaapi"),
                    encode: vec!["h264".into(), "hevc".into(), "jpeg".into()],
                    decode: vec!["h264".into(), "hevc".into(), "vp9".into(), "av1".into(), "jpeg".into()],
                    ready: true,
                    ..Gpu::default()
                },
                "amd" => Gpu {
                    pci,
                    vendor: "amd",
                    name: "AMD Radeon Pro W7600".into(),
                    driver: Some("amdgpu".into()),
                    render_node: render,
                    note: Some("VA-API not ready: install vainfo and the VA-API driver".into()),
                    ..Gpu::default()
                },
                _ => return None,
            })
        })
        .collect()
}

static CACHE: Mutex<Option<(Instant, Vec<Gpu>)>> = Mutex::new(None);

/// Ce qui ne change pas tant que le Core tourne (le nom d'une carte, ce que son pilote VA-API sait
/// faire) : demandé une fois (lspci, vainfo), pas à chaque relevé.
fn memo(key: String, f: impl FnOnce() -> Option<String>) -> Option<String> {
    static SEEN: Mutex<Option<std::collections::HashMap<String, Option<String>>>> = Mutex::new(None);
    if let Some(v) = SEEN.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(Default::default).get(&key) {
        return v.clone();
    }
    let v = f();
    SEEN.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(Default::default).insert(key, v.clone());
    v
}

/// Les GPU (relus au plus toutes les 3 s).
pub fn detect() -> Vec<Gpu> {
    if let Some(f) = std::env::var("STRATA_FAKE_GPU").ok().filter(|v| !v.trim().is_empty()) {
        return fake(&f);
    }
    let mut c = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((at, v)) = c.as_ref() {
        if at.elapsed() < Duration::from_secs(3) {
            return v.clone();
        }
    }
    let v = scan();
    *c = Some((Instant::now(), v.clone()));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codecs() {
        let v = "vainfo: Driver version: Intel iHD driver for Intel(R) Gen Graphics - 24.1.0\n\
                 VAProfileH264Main               :\tVAEntrypointVLD\n\
                 VAProfileH264Main               :\tVAEntrypointEncSliceLP\n\
                 VAProfileHEVCMain               :\tVAEntrypointVLD\n\
                 VAProfileAV1Profile0            :\tVAEntrypointVLD\n\
                 VAProfileNone                   :\tVAEntrypointVideoProc\n";
        let (enc, dec) = vaapi_codecs(v);
        assert_eq!(enc, ["h264"]);
        assert_eq!(dec, ["h264", "hevc", "av1"]);
        assert!(nvidia_codecs("NVIDIA L4").0.contains(&"av1".to_string()));
        assert!(!nvidia_codecs("Quadro RTX 4000").1.contains(&"av1".to_string()));
        assert!(same_bus("00000000:3B:00.0", "0000:3b:00.0"));
        let f = fake("nvidia,intel,nouveau");
        assert_eq!(f.len(), 3);
        assert!(f[0].ready && !f[2].ready);
    }
}
