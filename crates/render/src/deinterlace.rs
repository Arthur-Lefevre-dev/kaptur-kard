//! Deinterlace stage stub (Bob/Weave later when interlaced caps appear).

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DeinterlaceMode {
    #[default]
    Off,
    Bob,
    Weave,
}
