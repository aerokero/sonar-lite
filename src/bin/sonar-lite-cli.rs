// usage: sonar-lite-cli                                  -> status
//        sonar-lite-cli set <game|media|chat|aux|all> [--out <name>] [--gain 0..1] [--mute on|off]
//        sonar-lite-cli apps                              -> apps with audio sessions and where they go
//        sonar-lite-cli app <pid|exe> <game|media|chat|aux|default>
//        sonar-lite-cli eq <channel> [<band 1-10> [--on|--off] [--gain dB] [--freq Hz] [--q Q] [--type peaking|low|high]]
//        sonar-lite-cli preset <list | save|load <channel> <name>>
// Run from an elevated terminal (writes HKLM).
use sonar_lite::*;
use std::{env, process::exit};
use winreg::{enums::HKEY_LOCAL_MACHINE, RegKey};

fn short_name(n: &str) -> String { n.split(" (").next().unwrap_or(n).to_string() }

fn die(m: impl std::fmt::Display) -> ! { eprintln!("error: {m}"); exit(1) }

fn status(hklm: &RegKey) {
    let eps = endpoints(hklm);
    for c in read_all(hklm) {
        let dev = eps.iter().find(|e| e.guid == c.guid).map_or(format!("<unknown {}>", c.guid), |e| e.name.clone());
        println!("{:<6} -> {dev} | gain {:.2} | mute {}", c.key, c.gain, if c.mute { "on" } else { "off" });
    }
}

fn set(hklm: &RegKey, a: &[String]) {
    let which = a.first().map(String::as_str).unwrap_or_else(|| die("channel required"));
    let (mut out, mut gain, mut mute) = (None, None, None);
    let mut it = a[1..].iter();
    while let Some(f) = it.next() {
        let v = it.next().unwrap_or_else(|| die(format!("{f} needs a value")));
        match f.as_str() {
            "--out" => out = Some(v.to_lowercase()),
            "--gain" => gain = Some(v.parse::<f32>().ok().filter(|g| (0.0..=1.0).contains(g)).unwrap_or_else(|| die("--gain must be 0..1"))),
            "--mute" => mute = Some(match v.as_str() { "on" => true, "off" => false, _ => die("--mute on|off") }),
            _ => die(format!("unknown option {f}")),
        }
    }
    let target = out.map(|n| {
        let hits: Vec<_> = endpoints(hklm).into_iter().filter(|e| !e.virt && e.name.to_lowercase().contains(&n)).collect();
        match hits.len() {
            1 => hits[0].guid.clone(),
            0 => die(format!("no active output matching {n:?}")),
            _ => die(format!("{n:?} is ambiguous: {}", hits.iter().map(|h| h.name.as_str()).collect::<Vec<_>>().join("; "))),
        }
    });
    let mut chans: Vec<_> = read_all(hklm).into_iter().filter(|c| which == "all" || c.key == which).collect();
    if chans.is_empty() { die(format!("unknown channel {which:?}")) }
    for c in &mut chans {
        let key = c.key;
        let chk = |r: std::io::Result<()>| r.unwrap_or_else(|e| die(format!("{key}: {e} (elevated terminal?)")));
        if let Some(g) = &target { chk(c.set_out(hklm, g)) }
        if let Some(g) = gain { chk(c.set_gain(hklm, g)) }
        if let Some(m) = mute { chk(c.set_mute(hklm, m)) }
    }
    status(hklm);
}

fn apps(hklm: &RegKey) {
    let eps = endpoints(hklm);
    let pol = apps::Policy::new().unwrap_or_else(|e| die(e));
    for apps::Session { pid, exe, endpoint, .. } in apps::sessions() {
        let on = eps.iter().find(|e| e.guid == endpoint).map_or(endpoint.clone(), |e| short_name(&e.name));
        let g = pol.get(pid);
        let ch = CHANNELS.iter().map(|c| c.0).find(|k| g.is_some() && sonar_guid(&eps, k) == g);
        let to = ch.map(str::to_string).or_else(|| g.as_ref().map(|g| eps.iter().find(|e| &e.guid == g).map_or(g.clone(), |e| e.name.clone()))).unwrap_or("default".into());
        println!("{pid:>6}  {exe:<28} plays on {on:<24} (pin: {to})");
    }
}

fn app(hklm: &RegKey, a: &[String]) {
    let (Some(who), Some(to)) = (a.first(), a.get(1)) else { die("usage: app <pid|exe> <game|media|chat|aux|default>") };
    let guid = if to == "default" { None } else { Some(sonar_guid(&endpoints(hklm), to).unwrap_or_else(|| die(format!("unknown channel {to:?}")))) };
    let pol = apps::Policy::new().unwrap_or_else(|e| die(e));
    let hits: Vec<_> = apps::sessions().into_iter().filter(|s| who.parse() == Ok(s.pid) || s.exe.eq_ignore_ascii_case(who)).collect();
    if hits.is_empty() { die(format!("no audio session for {who:?}")) }
    for apps::Session { pid, exe, .. } in hits {
        pol.set(pid, guid.as_deref()).unwrap_or_else(|e| die(e));
        println!("{pid} {exe} -> {to}");
    }
}

fn eq(hklm: &RegKey, a: &[String]) {
    let key = a.first().map(String::as_str).unwrap_or_else(|| die("usage: eq <channel> [band opts]"));
    let c = read_all(hklm).into_iter().find(|c| c.key == key).unwrap_or_else(|| die(format!("unknown channel {key:?}")));
    if let Some(n) = a.get(1) {
        let i = n.parse::<usize>().ok().filter(|n| (1..=10).contains(n)).unwrap_or_else(|| die("band 1-10")) - 1;
        let mut b = c.band(hklm, i);
        let mut it = a[2..].iter();
        while let Some(f) = it.next() {
            let mut v = || it.next().and_then(|v| v.parse::<f32>().ok()).unwrap_or_else(|| die(format!("{f} needs a number")));
            match f.as_str() {
                "--on" => b.on = true,
                "--off" => b.on = false,
                "--gain" => b.gain = v(),
                "--freq" => b.freq = v(),
                "--q" => b.q = v(),
                "--type" => b.kind = match it.next().map(String::as_str) { Some("peaking") => 7, Some("low") => 8, Some("high") => 9, _ => die("--type peaking|low|high") },
                _ => die(format!("unknown option {f}")),
            }
        }
        c.set_band(hklm, i, &b).unwrap_or_else(|e| die(format!("{e} (elevated terminal?)")));
    }
    println!("{key} EQ {}", if c.eq_on(hklm) { "on" } else { "off" });
    for i in 0..10 {
        let b = c.band(hklm, i);
        let k = eq::KINDS.iter().find(|k| k.0 == b.kind).map_or(format!("type {}", b.kind), |k| k.1.to_string());
        println!("  {:>2} {} {k:<10} {:>7.0} Hz {:>+5.1} dB  Q {:.2}", i + 1, if b.on { "on " } else { "off" }, b.freq, b.gain, b.q);
    }
}

fn preset(hklm: &RegKey, a: &[String]) {
    match a.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["list"] => eq::list_presets().iter().for_each(|p| println!("{p}")),
        [op @ ("save" | "load"), ch, name] => {
            let c = read_all(hklm).into_iter().find(|c| c.key == ch).unwrap_or_else(|| die(format!("unknown channel {ch:?}")));
            if op == "save" { eq::save_preset(name, &c.snapshot(hklm)).unwrap_or_else(|e| die(e)) }
            else { c.apply(hklm, &eq::load_preset(name).unwrap_or_else(|e| die(e))).unwrap_or_else(|e| die(format!("{e} (elevated terminal?)"))) }
            println!("{op} {name} ok");
        }
        _ => die("usage: preset list | save <channel> <name> | load <channel> <name>"),
    }
}

fn main() {
    let a: Vec<String> = env::args().skip(1).collect();
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    match a.first().map(String::as_str) {
        None | Some("status") => status(&hklm),
        Some("set") => set(&hklm, &a[1..]),
        Some("apps") => apps(&hklm),
        Some("app") => app(&hklm, &a[1..]),
        Some("vad") => {
            // vad [on|off]: switch Sonar's virtual channel devices on (default) or off, like GG does
            let off = a.get(1).map(String::as_str) == Some("off");
            if off {
                for id in ["c3f8c90f-385b-455d-b8ef-55d5296f319f", "616607be-396f-4520-ac5b-b3ac4afa6e58", "aa848f1f-00ae-4ab3-a347-9d9d2eab8af0", "20ed651a-80a4-49b1-ada6-addcceafc8f4"] {
                    println!("{id}: {:?}", vad::set_visible(&format!("{{0.0.0.00000000}}.{{{id}}}"), false));
                }
            } else {
                println!("enabled {} endpoint(s); still disabled: {}", vad::ensure_enabled(&hklm), vad::disabled_channel_endpoints(&hklm).len());
            }
        }
        Some("autostart") => match a.get(1).map(String::as_str) {
            Some("on") => { autostart::set(true, true).unwrap_or_else(|e| die(e)); println!("autostart on") }
            Some("off") => { autostart::set(false, true).unwrap_or_else(|e| die(e)); println!("autostart off") }
            _ => println!("autostart {}", if autostart::enabled() { "on" } else { "off" }),
        },
        Some("refresh") => {
            let key = a.get(1).map(String::as_str).unwrap_or_else(|| die("refresh <channel>"));
            let pol = apps::Policy::new().unwrap_or_else(|e| die(e));
            println!("refreshed {} app(s)", apps::refresh_channel(&pol, &endpoints(&hklm), key));
        }
        Some("scan") => {
            // scan <channel> <ms>: flag modified-bit 0..287 one at a time, printing the time of each (to find which bit is which setting)
            let c = read_all(&hklm).into_iter().find(|c| Some(c.key) == a.get(1).map(String::as_str)).unwrap_or_else(|| die("scan <channel> <ms>"));
            let ms = a.get(2).and_then(|n| n.parse().ok()).unwrap_or(700);
            for i in 0..288 {
                c.notify(&hklm, &[i]).unwrap_or_else(|e| die(e));
                println!("{i} {}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis());
                std::thread::sleep(std::time::Duration::from_millis(ms));
            }
        }
        Some("notify") => {
            let c = read_all(&hklm).into_iter().find(|c| Some(c.key) == a.get(1).map(String::as_str)).unwrap_or_else(|| die("notify <channel> [bit...]"));
            let bits: Vec<usize> = a[2..].iter().filter_map(|b| b.parse().ok()).collect();
            c.notify(&hklm, &bits).unwrap_or_else(|e| die(e));
            println!("notified {}", c.key);
        }
        Some("eq") => eq(&hklm, &a[1..]),
        Some("preset") => preset(&hklm, &a[1..]),
        _ => die("usage: sonar-lite-cli [status] | set <game|media|chat|aux|all> [--out name] [--gain 0..1] [--mute on|off]"),
    }
}
