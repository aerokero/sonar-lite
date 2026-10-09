// Start Sonar Lite at logon. The app needs admin (HKLM writes), so a plain Run key would raise a UAC prompt at every login;
// a scheduled task with "highest privileges" starts it elevated and silently instead.
use std::{fs, io, os::windows::process::CommandExt, process::Command};

const TASK: &str = "SonarLite";
const NO_WINDOW: u32 = 0x0800_0000;

fn schtasks(args: &[&str]) -> io::Result<bool> {
    Ok(Command::new("schtasks").args(args).creation_flags(NO_WINDOW).output()?.status.success())
}

pub fn enabled() -> bool { schtasks(&["/Query", "/TN", TASK]).unwrap_or(false) }

pub fn set(on: bool, tray: bool) -> io::Result<()> {
    if !on { schtasks(&["/Delete", "/TN", TASK, "/F"])?; return Ok(()); }
    let exe = std::env::current_exe()?;
    let user = format!("{}\\{}", std::env::var("USERDOMAIN").unwrap_or_default(), std::env::var("USERNAME").unwrap_or_default());
    let xml = format!(r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <StartWhenAvailable>true</StartWhenAvailable>
    <Enabled>true</Enabled>
  </Settings>
  <Actions Context="Author"><Exec><Command>{}</Command>{args}</Exec></Actions>
</Task>"#, exe.display(), args = if tray { "<Arguments>--tray</Arguments>" } else { "" });
    let path = std::env::temp_dir().join("sonarlite_task.xml");
    // schtasks wants UTF-16 LE with a BOM
    let bytes: Vec<u8> = [0xFEFFu16].iter().copied().chain(xml.encode_utf16()).flat_map(u16::to_le_bytes).collect();
    fs::write(&path, bytes)?;
    let ok = schtasks(&["/Create", "/TN", TASK, "/XML", &path.to_string_lossy(), "/F"])?;
    let _ = fs::remove_file(&path);
    if ok { Ok(()) } else { Err(io::Error::other(crate::i18n::tr("schtasks /Create nie powiodło się (potrzebne uprawnienia administratora)"))) }
}
