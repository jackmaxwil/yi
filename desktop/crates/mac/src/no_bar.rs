//! Off the Mac there is no menu bar, so a status item is never made and every method is unreachable.

use crate::{Row, RowId, StatusError};

pub struct StatusItem(std::convert::Infallible);

impl StatusItem {
    pub fn new(_on_pick: impl Fn(RowId) + 'static) -> Result<Self, StatusError> {
        Err(StatusError::NoMenuBar)
    }

    pub fn set(&self, _title: &str, _rows: &[Row]) -> Result<(), StatusError> {
        match self.0 {}
    }

    pub fn title(&self) -> Option<String> {
        match self.0 {}
    }

    pub fn pick_row(&self, _index: usize) -> Result<(), StatusError> {
        match self.0 {}
    }
}
