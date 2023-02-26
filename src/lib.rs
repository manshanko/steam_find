use std::fs;
use std::fmt::Write;
use std::io;
use std::str::Chars;
use std::path::Path;
use std::path::PathBuf;

#[cfg(target_os = "windows")]
pub fn steam_dir() -> io::Result<PathBuf> {
    use std::mem;
    use std::ffi::c_void;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::ffi::OsStringExt;

    extern "system" {
        fn RegGetValueW(
            hkey: isize,
            lpsubkey: *const u16,
            lpvalue: *const u16,
            dwflags: u32,
            pdwtype: *mut u32,
            pvdata: *mut c_void,
            pcbdata: *mut u32,
        ) -> u32;
    }

    const HKEY_CURRENT_USER: isize = -2_147_483_647isize;
    const RRF_RT_REG_SZ: u32 = 2u32;

    const BUFFER_SIZE: usize = 1024;

    let mut buffer: [u16; BUFFER_SIZE] = [0; BUFFER_SIZE];
    let mut size = (BUFFER_SIZE * mem::size_of_val(&buffer[0])) as u32;
    let mut kind = 0;
    unsafe {
        if RegGetValueW(
            HKEY_CURRENT_USER,
            OsString::from("SOFTWARE\\Valve\\Steam\0").encode_wide().collect::<Vec<_>>().as_ptr() as *const _,
            OsString::from("SteamPath\0").encode_wide().collect::<Vec<_>>().as_ptr() as *const _,
            RRF_RT_REG_SZ,
            &mut kind,
            buffer.as_mut_ptr() as *mut _,
            &mut size,
        ) == 0 {
            let len = (size as usize - 1) / 2;
            let path = PathBuf::from(OsString::from_wide(&buffer[..len]));

            return Ok(path);
        }
    }

    Err(io::Error::new(io::ErrorKind::NotFound, "failed to find Steam"))
}

#[cfg(target_os = "macos")]
pub fn steam_dir() -> io::Result<PathBuf> {
    Ok(PathBuf::from("~/Library/Application Support/Steam"))
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
pub fn steam_dir() -> io::Result<PathBuf> {
    Ok(PathBuf::from("~/.steam/steam"))
}

pub fn steam_apps() -> io::Result<Vec<App>> {
    let mut steam = steam_dir()?;
    steam.push("steamapps");
    let lib = steam.join("libraryfolders.vdf");
    let buffer = fs::read_to_string(&lib)?;
    let lib = vdf_parse(buffer.chars())?;
    let mut libraries = Vec::new();
    for (_key, map) in lib["libraryfolders"].iter() {
        if let Some(path) = map["path"].as_str() {
            let path = path.replace(r"\\", r"\");
            let path = Path::new(&path).join("steamapps");
            libraries.push(path);
        }
    }

    let mut apps = Vec::new();
    for path in libraries.iter() {
        let root = path.join("common");
        for fd in fs::read_dir(path)? {
            let path = fd?.path();
            if path.extension().and_then(|os| os.to_str()) == Some("acf") {
                let buffer = fs::read_to_string(path)?;
                let ast = vdf_parse(buffer.chars())?;
                let state = &ast["AppState"];

                if let Some(app) = (|| {
                    Some(App {
                        app_id: state["appid"].as_int()? as u64,
                        size_on_disk: state["SizeOnDisk"].as_int()? as u64,
                        path: root.join(state["installdir"].as_str()?),
                        name: state["name"].as_str()?.to_string(),
                    })
                })() {
                    apps.push(app);
                }
            }
        }
    }
    apps.sort_unstable_by(|a, b| a.app_id.cmp(&b.app_id));
    Ok(apps)
}

pub fn get_steam_app(app_id: u64) -> io::Result<App> {
    let mut steam = steam_dir()?;
    steam.push("steamapps");
    let lib = steam.join("libraryfolders.vdf");
    let buffer = fs::read_to_string(&lib)?;
    let lib = vdf_parse(buffer.chars())?;
    for (_key, map) in lib["libraryfolders"].iter() {
        for (entry_app_id, _) in map["apps"].iter() {
            if let Ok(target_id) = u64::from_str_radix(entry_app_id, 10) {
                if target_id != app_id {
                    continue;
                }

                if let Some(path) = map["path"].as_str() {
                    let mut path = path.replace(r"\\", r"\");
                    write!(&mut path, "/steamapps/").unwrap();
                    let len = path.len();
                    write!(&mut path, "appmanifest_{target_id}.acf").unwrap();
                    let buffer = fs::read_to_string(&path)?;
                    path.truncate(len);
                    path.push_str("common/");
                    let ast = vdf_parse(buffer.chars())?;
                    let state = &ast["AppState"];

                    if let Some(app) = (|| Some(App {
                            app_id: state["appid"].as_int()? as u64,
                            size_on_disk: state["SizeOnDisk"].as_int()? as u64,
                            path: Path::new(&path).join(state["installdir"].as_str()?),
                            name: state["name"].as_str()?.to_string(),
                    }))() {
                        return Ok(app);
                    }
                }
            }
        }
    }
    Err(io::Error::new(io::ErrorKind::NotFound, "failed to find app"))
}

#[derive(Debug)]
pub struct App {
    pub app_id: u64,
    pub name: String,
    pub size_on_disk: u64,
    pub path: PathBuf,
}

#[derive(Debug)]
enum Value<'a> {
    Map(Vec<(&'a str, Value<'a>)>),
    Str(&'a str),
    Null,
}

impl<'a> Value<'a> {
    fn as_str(&self) -> Option<&'a str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    fn as_int(&self) -> Option<i64> {
        match self {
            Value::Str(s) => i64::from_str_radix(s, 10).ok(),
            _ => None,
        }
    }

    fn iter(&self) -> std::slice::Iter<'a, (&'a str, Value)> {
        match self {
            Value::Map(map) => map.iter(),
            _ => [].iter(),
        }
    }
}

impl<'a> std::ops::Index<&str> for Value<'a> {
    type Output = Value<'a>;

    fn index(&self, key: &str) -> &Self::Output {
        match self {
            Value::Map(map) => map
                .iter()
                .find(|(probe, _)| probe.eq_ignore_ascii_case(key))
                .map(|res| &res.1)
                .unwrap_or(&Value::Null),
            _ => &Value::Null,
        }
    }
}

fn vdf_parse<'a>(mut stream: Chars<'a>) -> io::Result<Value<'a>> {
    fn parse_str<'a>(chars: &mut Chars<'a>) -> io::Result<&'a str> {
        let buf = chars.as_str();
        let mut len = 0;
        while let Some(next) = chars.next() {
            if next == '"' {
                break;
            } else {
                len += next.len_utf8();
            }
        }
        Ok(&buf[..len])
    }

    let mut stack: Vec<(Vec<(&str, Value)>, &str)> = Vec::with_capacity(16);
    let mut map = Vec::new();
    let mut key = None;
    while let Some(start) = stream.next() {
        if start.is_ascii_whitespace() {
            continue;
        }

        if key.is_none() {
            if start == '"' {
                key = Some(parse_str(&mut stream).unwrap());
            } else if start == '}' {
                let (mut parent, key) = stack.pop().unwrap();
                parent.push((key, Value::Map(map)));
                map = parent;
            } else {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "unexpected token while parsing"));
            }
        } else if let Some(key) = key.take() {
            if start == '"' {
                map.push((key, Value::Str(parse_str(&mut stream).unwrap())));
            } else if start == '{' {
                map.sort_unstable_by(|a, b| a.0.cmp(&b.0));
                stack.push((std::mem::take(&mut map), key));
            } else {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "unexpected token while parsing"));
            }
        } else {
            unreachable!();
        }
    }
    map.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    Ok(Value::Map(map))
}



#[cfg(test)]
mod test {
    #[test]
    fn parse() {
        let ast = crate::vdf_parse(r#"
            "AppState"
            {
                "appid"     "55500"
                "name"      "Test Game"
                "UserConfig"
                {
                    "language"      "english"
                }
            }
        "#.chars()).unwrap();

        assert_eq!(ast["AppState"]["appid"].as_int(), Some(55500));
        assert_eq!(ast["AppState"]["name"].as_str(), Some("Test Game"));
        assert_eq!(ast["AppState"]["UserConfig"]["language"].as_str(), Some("english"));
    }
}