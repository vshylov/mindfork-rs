//! Слой `entities` (FSD): доменные типы без I/O (`Profile`, `Chat`, `Message`,
//! `Note`, `RagDocument`, `SamplingConfig`, …). См. spec §4.2, §5.1.
//!
//! На M1 объявлён `sampling`; остальные сущности — на M2.

pub mod sampling;
