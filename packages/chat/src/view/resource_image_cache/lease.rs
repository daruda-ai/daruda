//! Pins shared by preview cards and their modal consumers.
use daruda_ui::CachedImage;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ImageState {
    Resident,
    Retired,
    Released,
}

pub(super) type Maintenance = Rc<RefCell<Option<Rc<dyn Fn(Option<CachedImage>)>>>>;

pub(super) struct ImageOwner {
    pub(super) image: CachedImage,
    pub(super) state: Cell<ImageState>,
    pub(super) modal_pins: Cell<usize>,
    pub(super) maintenance: Maintenance,
}
impl Drop for ImageOwner {
    fn drop(&mut self) {
        if self.state.get() != ImageState::Released {
            let callback = self.maintenance.borrow().clone();
            if let Some(callback) = callback {
                callback(Some(self.image.clone()));
            }
        }
    }
}

/// An explicit consumer pin. Click handlers and lightboxes hold this alongside
/// their bitmap; retirement cannot free its CPU bytes or atlas tile meanwhile.
#[derive(Clone)]
pub(crate) struct ResourceImageLease(pub(super) Rc<ImageOwner>);
impl ResourceImageLease {
    pub(crate) fn into_modal(self) -> ResourceModalLease {
        self.0.modal_pins.set(self.0.modal_pins.get() + 1);
        ResourceModalLease(self)
    }
}

/// A modal keeps its bitmap across paints; unlike a card's frame pin it
/// cannot contribute to projected reclamation on the next repaint.
pub(crate) struct ResourceModalLease(ResourceImageLease);
impl std::ops::Deref for ResourceModalLease {
    type Target = CachedImage;
    fn deref(&self) -> &CachedImage {
        &self.0
    }
}
impl Drop for ResourceModalLease {
    fn drop(&mut self) {
        self.0.0.modal_pins.set(self.0.0.modal_pins.get() - 1);
    }
}

impl std::ops::Deref for ResourceImageLease {
    type Target = CachedImage;
    fn deref(&self) -> &CachedImage {
        &self.0.image
    }
}
impl Drop for ResourceImageLease {
    fn drop(&mut self) {
        if self.0.state.get() == ImageState::Retired && Rc::strong_count(&self.0) == 2 {
            let callback = self.0.maintenance.borrow().clone();
            if let Some(callback) = callback {
                callback(None);
            }
        }
    }
}

#[cfg(test)]
mod tests;
