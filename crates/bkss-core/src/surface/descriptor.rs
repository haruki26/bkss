use gpui::{App, DisplayId, Entity, Pixels, Render, Size, Window};
use std::{error::Error, fmt, time::Duration};

use super::DEFAULT_DISPLAY_READINESS_TIMEOUT;

pub(super) type ContentFactory<V> = Box<dyn FnOnce(&mut Window, &mut App) -> Entity<V>>;

/// An inert declaration for one non-interactive layer-shell surface.
///
/// `size` is expressed in GPUI logical pixels and each dimension must be at
/// least one pixel. The content factory is called at most once, after the
/// requested display appears. Register this during shell setup with
/// [`RuntimeHandle::mount_surface`](crate::RuntimeHandle::mount_surface).
pub struct SurfaceDescriptor<V: Render + 'static> {
    pub(crate) display_id: DisplayId,
    pub(crate) size: Size<Pixels>,
    pub(crate) readiness_timeout: Duration,
    pub(crate) content: ContentFactory<V>,
}

impl<V: Render + 'static> SurfaceDescriptor<V> {
    /// Creates a descriptor for a single display and a finite size of at least one logical pixel.
    /// The fixed namespace is `bkss-core-surface`.
    pub fn new(
        display_id: DisplayId,
        size: Size<Pixels>,
        content: impl FnOnce(&mut Window, &mut App) -> Entity<V> + 'static,
    ) -> Result<Self, SurfaceDescriptorError> {
        if ![size.width, size.height]
            .into_iter()
            .all(|dimension| f32::from(dimension).is_finite() && f32::from(dimension) >= 1.0)
        {
            return Err(SurfaceDescriptorError::InvalidSize(size));
        }
        Ok(Self {
            display_id,
            size,
            readiness_timeout: DEFAULT_DISPLAY_READINESS_TIMEOUT,
            content: Box::new(content),
        })
    }

    /// Sets the readiness deadline. A zero timeout fails immediately, even if
    /// the display is already present when readiness is first checked.
    pub fn readiness_timeout(mut self, timeout: Duration) -> Self {
        self.readiness_timeout = timeout;
        self
    }
}

#[derive(Clone, Copy, Debug)]
pub enum SurfaceDescriptorError {
    InvalidSize(Size<Pixels>),
}

impl fmt::Display for SurfaceDescriptorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSize(size) => write!(
                f,
                "surface dimensions must be finite and at least one logical pixel, got {:?}",
                size
            ),
        }
    }
}

impl Error for SurfaceDescriptorError {}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, DisplayId, Size, px};
    use std::time::Duration;

    #[test]
    fn invalid_size_is_rejected_when_descriptor_is_built() {
        let result = SurfaceDescriptor::<gpui::EmptyView>::new(
            DisplayId::new(1),
            Size::new(px(0.), px(10.)),
            |_, cx| cx.new(|_| gpui::EmptyView),
        );
        assert!(result.is_err());
    }

    #[test]
    fn readiness_default_is_five_seconds() {
        assert_eq!(DEFAULT_DISPLAY_READINESS_TIMEOUT, Duration::from_secs(5));
    }
}
