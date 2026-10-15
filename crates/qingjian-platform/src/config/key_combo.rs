use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::modifiers::Modifiers;

/// 修饰键 + 一个字母键的组合，配置里写成 `control+option+t`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct KeyCombo {
    /// 修饰键，至少一个。
    pub modifiers: Modifiers,

    /// 字母、数字或句号键（小写）。
    pub key: char,
}

impl KeyCombo {
    pub const TRANSLATE_DEFAULT: Self = Self {
        modifiers: Modifiers {
            option: true,
            shift: false,
            control: true,
            command: false,
        },
        key: 't',
    };

    /// 暂存区译文上屏。`key == '\n'` 表示回车。
    pub const STAGING_TRANSLATION_COMMIT: Self = Self {
        modifiers: Modifiers::SHIFT,
        key: '\n',
    };

    /// 是不是回车，而不是字母或数字。
    pub fn is_enter(self) -> bool {
        self.key == '\n'
    }

    /// 配置文件里的写法。
    pub fn key_string(&self) -> String {
        let key = if self.is_enter() {
            "enter".to_owned()
        } else {
            self.key.to_string()
        };
        format!("{}+{key}", self.modifiers.key())
    }

    /// 给人看的写法：`⌃⌥T`，回车写成 `Enter`。
    pub fn label(&self) -> String {
        let key = if self.is_enter() {
            "Enter".to_owned()
        } else {
            self.key.to_ascii_uppercase().to_string()
        };
        format!("{}{key}", self.modifiers.label())
    }
}

impl Default for KeyCombo {
    fn default() -> Self {
        Self::TRANSLATE_DEFAULT
    }
}

impl FromStr for KeyCombo {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (modifiers, key) = text
            .trim()
            .rsplit_once('+')
            .ok_or_else(|| format!("expected modifiers+key, got {text:?}"))?;
        let key = if key.trim().eq_ignore_ascii_case("enter") {
            '\n'
        } else {
            let mut chars = key.trim().chars();
            let (Some(key), None) = (chars.next(), chars.next()) else {
                return Err(format!("key must be a single character: {key:?}"));
            };
            if !key.is_ascii_alphanumeric() && key != '.' {
                return Err(format!("key must be a letter, digit or period: {key:?}"));
            }
            key
        };
        Ok(Self {
            modifiers: modifiers.parse()?,
            key: key.to_ascii_lowercase(),
        })
    }
}

impl TryFrom<String> for KeyCombo {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

impl From<KeyCombo> for String {
    fn from(combo: KeyCombo) -> Self {
        combo.key_string()
    }
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.key_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_labels() {
        let combo: KeyCombo = "control+option+t".parse().unwrap();
        assert_eq!(combo, KeyCombo::TRANSLATE_DEFAULT);
        assert_eq!(combo.label(), "⌃⌥T");
        assert_eq!(combo.key_string(), "control+option+t");
        let enter: KeyCombo = "shift+enter".parse().unwrap();
        assert_eq!(enter, KeyCombo::STAGING_TRANSLATION_COMMIT);
        assert_eq!(enter.label(), "⇧Enter");
        assert_eq!(enter.key_string(), "shift+enter");
        assert!("t".parse::<KeyCombo>().is_err());
        assert!("option+tt".parse::<KeyCombo>().is_err());
        assert!("option+-".parse::<KeyCombo>().is_err());
        for option in ["control+shift+t", "control+option+e", "shift+command+9"] {
            assert_eq!(option.parse::<KeyCombo>().unwrap().key_string(), option);
        }
    }
}
