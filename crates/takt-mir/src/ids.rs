//! Indizes in die Tabellen der MIR. Jeder Name des Programms ist nach dem
//! Lowering ein Index; Namen und Spannen bleiben als Seitenfelder der Eintraege
//! fuer Diagnosen und Telemetrie erhalten (plan/mir.md, Abschnitt 1).

macro_rules! ids {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {
        $(
            $(#[$doc])*
            #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
            pub struct $name(pub u32);

            impl $name {
                /// Index als `usize`.
                pub fn index(self) -> usize {
                    self.0 as usize
                }
            }
        )*
    };
}

ids! {
    /// Eintrag in `Program::types`.
    TypeId,
    /// Eintrag in `Program::units`.
    UnitId,
    /// Eintrag in `Program::enums`.
    EnumId,
    /// Eintrag in `Program::records`.
    RecordId,
    /// Eintrag in `Program::fns`.
    FnId,
    /// Eintrag in `Program::natives`.
    NativeId,
    /// Eintrag in `Program::blocks`.
    BlockId,
    /// Eintrag in `Program::machines`.
    MachineId,
    /// Eintrag in `Program::channels`.
    ChannelId,
    /// Eintrag in `Program::streams` (interne Streams).
    StreamId,
    /// Eintrag in `Program::params`.
    ParamId,
    /// Eintrag in `Program::profiles`.
    ProfileId,
    /// Eintrag in `Program::commands`.
    CommandId,
    /// Eintrag in `Program::nodes`.
    NodeId,
    /// Eintrag in `Program::properties`.
    PropertyId,
    /// Eintrag in `Program::campaigns`.
    CampaignId,
    /// Eintrag in `Program::triggers`.
    TriggerId,
    /// Eintrag in `Program::ports`.
    PortId,
    /// Eintrag in `Machine::states`.
    StateId,
    /// Eintrag in `Machine::vars` beziehungsweise `Fn::locals`.
    VarId,
    /// Eintrag in `Machine::signals`.
    SignalId,
    /// Eintrag in `Layout::viol_sites` (Bestaetigungszaehler eines `check … for d`).
    SiteId,
    /// Eintrag in `Layout::every_counters`.
    CounterId,
}
