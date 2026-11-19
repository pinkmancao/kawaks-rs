use std::{collections::HashMap, io, path::PathBuf, sync::OnceLock};

use super::pad_setup;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Language {
    #[default]
    English,
    Chinese,
}

impl Language {
    fn path() -> PathBuf {
        pad_setup::profile_path().with_file_name("language")
    }

    pub fn load() -> Self {
        match std::fs::read_to_string(Self::path()).as_deref() {
            Ok("zh-Hans") => Self::Chinese,
            _ => Self::English,
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::English => Self::Chinese,
            Self::Chinese => Self::English,
        }
    }

    pub fn save(self) -> io::Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(
            path,
            if self == Self::Chinese {
                "zh-Hans"
            } else {
                "en"
            },
        )
    }

    pub fn tr(self, english: &str) -> &str {
        if self == Self::English {
            return english;
        }
        match english {
            "ROM LIBRARY" => "游戏库",
            "Search name or system" => "搜索游戏名、短名或平台",
            "No matching ROMs" => "没有匹配的游戏",
            "ROM MISSING" => "缺少 ROM",
            "N/A" => "暂不支持",
            "No compatible driver for this ROM." => "当前版本还没有此 ROM 的兼容驱动。",
            "ROM" => "ROM 文件",
            "Choose ZIP" => "选择 ZIP",
            "Open arcade ROM" => "选择游戏 ROM",
            "ROM archive" => "游戏 ROM 压缩包",
            "STARTING STAGE" => "起始关卡",
            "Normal start" => "正常开局",
            "Open KWK4 state" => "打开 KWK4 存档",
            "KWK4 save" => "KWK4 存档",
            "NEO GEO BIOS" => "Neo Geo BIOS",
            "Beside ROM by default" => "默认使用 ROM 同目录的 BIOS",
            "Neo Geo games only" => "仅 Neo Geo 游戏需要",
            "Open Neo Geo BIOS" => "选择 Neo Geo BIOS",
            "BIOS archive" => "BIOS 压缩包",
            "INPUT CONFIG" => "输入配置",
            "Default controls" => "默认操控",
            "Open input configuration" => "打开输入配置文件",
            "CONFIGURE" => "配置摇杆",
            "CHOOSE" => "选择",
            "OPEN" => "打开",
            "Local" => "本地",
            "Host LAN" => "创建房间",
            "Join LAN" => "加入房间",
            "AUDIO OFF" => "声音关闭",
            "AUDIO ON" => "声音开启",
            "CANCEL" => "取消",
            "PLAY" => "开始",
            "LAN connection ended" => "联机连接已结束",
            "The other player disconnected." => "对方玩家已断开",
            "Waiting for player 2" => "等待第二位玩家",
            "Connecting to host" => "正在连接主机",
            "Enter a numeric IP and port" => "请输入数字 IP 地址和端口",
            "Joystick settings saved and applied" => "摇杆配置已保存并应用",
            "Kawaks Rust - Starting Stage" => "Kawaks Rust - 起始关卡",
            "Choose a saved checkpoint, or start normally." => "可从存档检查点继续，也可正常开局。",
            "Select a stage, then coin, start and choose your fighter." => {
                "选好关卡后，照常投币、开始并选人。"
            }
            "Esc closes" => "按 Esc 关闭",
            "Stage 1" => "第一关",
            "Stage 2" => "第二关",
            "Stage 3" => "第三关",
            "Stage 4" => "第四关",
            "Stage 5 - Part 1" => "第五关（上）",
            "Stage 5 - Part 2" => "第五关（下）",
            "Stage 6" => "第六关",
            "Stage 7" => "第七关",
            "Stage 8" => "第八关",
            "Stage 9" => "第九关",
            "Meimei ambush" => "美美刺客关",
            "Final - Cao Cao" => "最终关：曹操",
            "Bonus 1 - Eating" => "奖励关一：吃肉",
            "Bonus 2 - Duel" => "奖励关二：对打",
            "Saved slot" => "存档槽",
            "Checkpoint" => "检查点",
            "Kawaks Rust - Joystick Setup" => "Kawaks Rust - 摇杆配置",
            "JOYSTICK SETUP" => "摇杆配置",
            "UP" => "上",
            "DOWN" => "下",
            "LEFT" => "左",
            "RIGHT" => "右",
            "BUTTON 1" => "动作键 1",
            "BUTTON 2" => "动作键 2",
            "BUTTON 3" => "动作键 3",
            "BUTTON 4" => "动作键 4",
            "BUTTON 5" => "动作键 5",
            "BUTTON 6" => "动作键 6",
            "START" => "开始",
            "COIN" => "投币",
            "HORIZONTAL AXIS" => "水平轴",
            "VERTICAL AXIS" => "垂直轴",
            "D-PAD HAT" => "方向帽",
            "DEAD ZONE" => "死区",
            "No joystick detected" => "未检测到摇杆",
            "Connect a joystick for this player." => "请为该玩家连接摇杆。",
            "Move the axis, or press Esc to cancel." => "拨动摇杆轴，或按 Esc 取消。",
            "Move the D-pad, or press Esc to cancel." => "拨动方向帽，或按 Esc 取消。",
            "Press the joystick button, or Esc to cancel." => "按下摇杆按钮，或按 Esc 取消。",
            "Click a row to bind. X clears it. Active inputs light up." => {
                "点击一行开始绑定，点 X 清除。收到输入时该行高亮。"
            }
            "None" => "无",
            "Waiting..." => "等待输入",
            "DEFAULTS" => "恢复默认",
            "SAVE & APPLY" => "保存并应用",
            "Could not save" => "保存失败",
            "Paused (Enter to resume)" => "已暂停（按回车继续）",
            _ => english,
        }
    }

    pub fn title(self, id: &str) -> &str {
        titles()
            .get(id)
            .map_or(id, |&(en, zh)| if self == Self::Chinese { zh } else { en })
    }
}

fn titles() -> &'static HashMap<&'static str, (&'static str, &'static str)> {
    static TITLES: OnceLock<HashMap<&'static str, (&'static str, &'static str)>> = OnceLock::new();
    TITLES.get_or_init(|| {
        include_str!("titles.tsv")
            .lines()
            .filter_map(|line| {
                let mut fields = line.split('\t');
                Some((fields.next()?, (fields.next()?, fields.next()?)))
            })
            .collect()
    })
}

pub(super) fn wide_glyph(ch: char) -> Option<&'static [u8]> {
    const SIZE: usize = 32;
    const ATLAS: &[u8] = include_bytes!("kanji.glyphs");
    let mut lo = 0;
    let mut hi = ATLAS.len() / SIZE;
    while lo < hi {
        let mid = (lo + hi) / 2;
        let glyph = &ATLAS[mid * SIZE..(mid + 1) * SIZE];
        let key = u32::from_le_bytes(glyph[..4].try_into().unwrap());
        match u32::from(ch).cmp(&key) {
            std::cmp::Ordering::Less => hi = mid,
            std::cmp::Ordering::Greater => lo = mid + 1,
            std::cmp::Ordering::Equal => return Some(&glyph[4..]),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_locales_and_glyphs_are_available() {
        assert_eq!(Language::Chinese.title("wofa"), "三国志Ⅱ（亚洲版）");
        assert_eq!(Language::English.title("dino"), "Cadillacs and Dinosaurs");
        assert_eq!(Language::Chinese.title("unknown"), "unknown");
        for &(_, title) in titles().values() {
            assert!(
                title
                    .chars()
                    .filter(|ch| !ch.is_ascii())
                    .all(|ch| wide_glyph(ch).is_some()),
                "{title}"
            );
        }
    }
}
