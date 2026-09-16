//! Das eingebaute Geraet `sys` (12.7, 7.4): die System-Channels, die
//! jede Plattform anbietet. Pruefung 60 kennt sie ohne
//! Hardware-Konfiguration — eine Konfiguration muss sie nicht wiederholen,
//! und ein Tippfehler (`sys/image_stat`) faellt auf.

use crate::program::Direction;

/// Der Typ eines System-Channels, wie das Programm ihn deklarieren muss.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SysType {
    /// Ein vordefiniertes Enum.
    Enum(&'static str),
    /// Ein vordefiniertes Record.
    Record(&'static str),
    /// `int`, jede Range.
    Int,
    /// `u8`.
    U8,
    /// `bool`.
    Bool,
    /// `[N] bool`.
    BoolArray(u32),
    /// `Duration`.
    Duration,
}

impl SysType {
    /// Der Typ, wie er im Programm steht.
    pub fn name(self) -> String {
        match self {
            SysType::Enum(n) | SysType::Record(n) => n.to_string(),
            SysType::Int => "int".into(),
            SysType::U8 => "u8".into(),
            SysType::Bool => "bool".into(),
            SysType::BoolArray(n) => format!("[{n}] bool"),
            SysType::Duration => "Duration".into(),
        }
    }
}

/// Ein Kanal des Geraets `sys`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SysChannel {
    /// Die Adresse, `sys/…`.
    pub address: &'static str,
    /// Richtung aus Sicht des Programms.
    pub dir: Direction,
    /// Typ.
    pub ty: SysType,
}

const fn sys(address: &'static str, dir: Direction, ty: SysType) -> SysChannel {
    SysChannel { address, dir, ty }
}

/// Die Kanaele des Geraets: die fuenf aus 12.7, die vier Start-Channels
/// und die Wanduhr (7.4).
pub const SYS: [SysChannel; 10] = [
    sys("sys/boot_reason", Direction::Input, SysType::Enum("BootReason")),
    sys("sys/image_state", Direction::Input, SysType::Enum("ImageState")),
    sys("sys/reset_count", Direction::Input, SysType::Int),
    sys("sys/image_confirm", Direction::Output, SysType::Bool),
    sys("sys/reboot", Direction::Output, SysType::Enum("RebootCmd")),
    sys("sys/efuse", Direction::Input, SysType::Record("EfuseBlock")),
    sys("sys/image_confirmed", Direction::Input, SysType::BoolArray(2)),
    sys("sys/jump", Direction::Output, SysType::U8),
    sys("sys/efuse_burn", Direction::Output, SysType::Enum("EfuseCmd")),
    sys("sys/clock", Direction::Input, SysType::Duration),
];

/// Der Kanal zu einer Adresse, wenn sie zum Geraet gehoert.
pub fn channel(address: &str) -> Option<&'static SysChannel> {
    SYS.iter().find(|c| c.address == address)
}

/// Gehoert die Adresse zum Geraet — auch als Tippfehler?
pub fn is_sys(address: &str) -> bool {
    address.starts_with("sys/")
}
