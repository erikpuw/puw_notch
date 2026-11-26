use std::cell::RefCell;

use windows::core::{implement, HSTRING, Interface, PCWSTR, Ref, Result};
use windows::Foundation::{IPropertyValue, PropertyValue};
use windows::Graphics::Effects::{IGraphicsEffect, IGraphicsEffect_Impl, IGraphicsEffectSource, IGraphicsEffectSource_Impl};
use windows::Graphics::{IGeometrySource2D, IGeometrySource2D_Impl};
use windows::System::DispatcherQueueController;
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::UI::Composition::{
    CompositionEffectBrush, CompositionEffectSourceParameter, CompositionPath, CompositionStretch,
    Compositor, ContainerVisual, SpriteVisual,
};
use windows::Win32::Foundation::{E_INVALIDARG, HWND};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_BORDER_MODE_HARD, D2D1_COLOR_F, D2D1_COLORMATRIX_ALPHA_MODE_PREMULTIPLIED,
    D2D1_COMPOSITE_MODE_SOURCE_OVER,
};
use windows::Win32::Graphics::Direct2D::{
    CLSID_D2D1ColorMatrix, CLSID_D2D1Composite, CLSID_D2D1GaussianBlur, CLSID_D2D1Saturation,
    D2D1_COLORMATRIX_PROP_ALPHA_MODE, D2D1_COLORMATRIX_PROP_CLAMP_OUTPUT,
    D2D1_COLORMATRIX_PROP_COLOR_MATRIX, D2D1_COMPOSITE_PROP_MODE,
    D2D1_GAUSSIANBLUR_OPTIMIZATION_BALANCED, D2D1_GAUSSIANBLUR_PROP_BORDER_MODE,
    D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION, D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION,
    D2D1_SATURATION_PROP_SATURATION, ID2D1Factory, ID2D1Geometry, ID2D1PathGeometry,
};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_HOSTBACKDROPBRUSH};
use windows::Win32::Graphics::Dxgi::IDXGISwapChain1;
use windows::Win32::System::WinRT::Composition::{ICompositorDesktopInterop, ICompositorInterop};
use windows::Win32::System::WinRT::Graphics::Direct2D::{
    GRAPHICS_EFFECT_PROPERTY_MAPPING, GRAPHICS_EFFECT_PROPERTY_MAPPING_DIRECT,
    IGeometrySource2DInterop, IGeometrySource2DInterop_Impl, IGraphicsEffectD2D1Interop,
    IGraphicsEffectD2D1Interop_Impl,
};
use windows::Win32::System::WinRT::{
    CreateDispatcherQueueController, DispatcherQueueOptions, DQTAT_COM_NONE, DQTYPE_THREAD_CURRENT,
};
use windows_numerics::{Vector2, Vector3};

/// No fixed dark veil over the frosted backdrop (matches the puwbar Frosted taskbar pill, which is lit instead of smoked).
pub(super) const PAGE_GLASS_ALPHA: f32 = 0.0;

/// Direct Gaussian blur radius (no hidden DPI multiplier).
const GLASS_BLUR: f32 = 5.0;

/// Subtle backdrop saturation boost so blurred colors stay vivid.
const GLASS_SATURATION: f32 = 1.0;

/// Auto-contrast: 0 below 50% luma (keeps dark & rich colorful wallpapers vivid),
/// deepens bright/white areas cleanly (~33% at 70% luma, ~82% on pure white) so white text stays crisp.
const AUTO_CONTRAST_SLOPE: f32 = 1.5;
const AUTO_CONTRAST_THRESHOLD: f32 = 0.6;

thread_local! {
    static DISPATCHER: RefCell<Option<DispatcherQueueController>> = const { RefCell::new(None) };
}

fn ensure_dispatcher() -> Result<()> {
    DISPATCHER.with(|cell| {
        if cell.borrow().is_some() {
            return Ok(());
        }
        let opts = DispatcherQueueOptions {
            dwSize: std::mem::size_of::<DispatcherQueueOptions>() as u32,
            threadType: DQTYPE_THREAD_CURRENT,
            apartmentType: DQTAT_COM_NONE,
        };
        // Ignore error if the thread already owns a DispatcherQueue; Compositor::new will validate.
        if let Ok(ctrl) = unsafe { CreateDispatcherQueueController(opts) } {
            *cell.borrow_mut() = Some(ctrl);
        }
        Ok(())
    })
}

pub(super) fn page_bg_alpha(panel_ok: bool, open: f32, glass_active: bool) -> f32 {
    if !panel_ok || !glass_active || open <= 0.0 {
        return 1.0;
    }
    1.0 - (1.0 - PAGE_GLASS_ALPHA) * open.clamp(0.0, 1.0)
}

/// Lifts muted neutral grey text (`0.18..0.99`) to translucent white over frosted glass so
/// secondary labels and out-of-month calendar dates stay legible, while leaving dark-on-white
/// pill text (`< 0.18`) and colored accents untouched.
pub(super) fn vibrant_text(c: D2D1_COLOR_F, on_glass: bool) -> D2D1_COLOR_F {
    if !on_glass {
        return c;
    }
    let neutral = (c.r - c.g).abs() < 0.02 && (c.g - c.b).abs() < 0.02;
    if neutral && c.r >= 0.18 && c.r < 0.99 {
        D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: (0.42 + 0.58 * c.r).min(1.0) * c.a }
    } else {
        c
    }
}

#[implement(IGraphicsEffect, IGraphicsEffectSource, IGraphicsEffectD2D1Interop)]
struct GaussianBlurEffect {
    name: RefCell<HSTRING>,
    blur: f32,
    source: IGraphicsEffectSource,
}

impl IGraphicsEffectSource_Impl for GaussianBlurEffect_Impl {}

impl IGraphicsEffect_Impl for GaussianBlurEffect_Impl {
    fn Name(&self) -> Result<HSTRING> {
        Ok(self.name.borrow().clone())
    }
    fn SetName(&self, name: &HSTRING) -> Result<()> {
        *self.name.borrow_mut() = name.clone();
        Ok(())
    }
}

impl IGraphicsEffectD2D1Interop_Impl for GaussianBlurEffect_Impl {
    fn GetEffectId(&self) -> Result<windows::core::GUID> {
        Ok(CLSID_D2D1GaussianBlur)
    }

    fn GetNamedPropertyMapping(
        &self,
        _name: &PCWSTR,
        _index: *mut u32,
        mapping: *mut GRAPHICS_EFFECT_PROPERTY_MAPPING,
    ) -> Result<()> {
        if !mapping.is_null() {
            unsafe { *mapping = GRAPHICS_EFFECT_PROPERTY_MAPPING_DIRECT };
        }
        Err(E_INVALIDARG.into())
    }

    fn GetPropertyCount(&self) -> Result<u32> {
        Ok(3)
    }

    fn GetProperty(&self, index: u32) -> Result<IPropertyValue> {
        let val = match index {
            i if i == D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32 => {
                PropertyValue::CreateSingle(self.blur)?
            }
            i if i == D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION.0 as u32 => {
                PropertyValue::CreateUInt32(D2D1_GAUSSIANBLUR_OPTIMIZATION_BALANCED.0 as u32)?
            }
            i if i == D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32 => {
                PropertyValue::CreateUInt32(D2D1_BORDER_MODE_HARD.0 as u32)?
            }
            _ => return Err(E_INVALIDARG.into()),
        };
        val.cast()
    }

    fn GetSource(&self, index: u32) -> Result<IGraphicsEffectSource> {
        if index == 0 {
            Ok(self.source.clone())
        } else {
            Err(E_INVALIDARG.into())
        }
    }

    fn GetSourceCount(&self) -> Result<u32> {
        Ok(1)
    }
}

#[implement(IGraphicsEffect, IGraphicsEffectSource, IGraphicsEffectD2D1Interop)]
struct SaturationEffect {
    name: RefCell<HSTRING>,
    saturation: f32,
    source: IGraphicsEffectSource,
}

impl IGraphicsEffectSource_Impl for SaturationEffect_Impl {}

impl IGraphicsEffect_Impl for SaturationEffect_Impl {
    fn Name(&self) -> Result<HSTRING> {
        Ok(self.name.borrow().clone())
    }
    fn SetName(&self, name: &HSTRING) -> Result<()> {
        *self.name.borrow_mut() = name.clone();
        Ok(())
    }
}

impl IGraphicsEffectD2D1Interop_Impl for SaturationEffect_Impl {
    fn GetEffectId(&self) -> Result<windows::core::GUID> {
        Ok(CLSID_D2D1Saturation)
    }

    fn GetNamedPropertyMapping(
        &self,
        _name: &PCWSTR,
        _index: *mut u32,
        mapping: *mut GRAPHICS_EFFECT_PROPERTY_MAPPING,
    ) -> Result<()> {
        if !mapping.is_null() {
            unsafe { *mapping = GRAPHICS_EFFECT_PROPERTY_MAPPING_DIRECT };
        }
        Err(E_INVALIDARG.into())
    }

    fn GetPropertyCount(&self) -> Result<u32> {
        Ok(1)
    }

    fn GetProperty(&self, index: u32) -> Result<IPropertyValue> {
        if index == D2D1_SATURATION_PROP_SATURATION.0 as u32 {
            PropertyValue::CreateSingle(self.saturation)?.cast()
        } else {
            Err(E_INVALIDARG.into())
        }
    }

    fn GetSource(&self, index: u32) -> Result<IGraphicsEffectSource> {
        if index == 0 {
            Ok(self.source.clone())
        } else {
            Err(E_INVALIDARG.into())
        }
    }

    fn GetSourceCount(&self) -> Result<u32> {
        Ok(1)
    }
}

#[implement(IGraphicsEffect, IGraphicsEffectSource, IGraphicsEffectD2D1Interop)]
struct LumaDarkenEffect {
    name: RefCell<HSTRING>,
    matrix: [f32; 20],
    source: IGraphicsEffectSource,
}

impl LumaDarkenEffect {
    fn new(slope: f32, threshold: f32, source: IGraphicsEffectSource) -> Self {
        let mut matrix = [0.0f32; 20];
        matrix[3] = slope * 0.2126;
        matrix[7] = slope * 0.7152;
        matrix[11] = slope * 0.0722;
        matrix[19] = -slope * threshold;
        Self { name: RefCell::new(HSTRING::from("Luma")), matrix, source }
    }
}

impl IGraphicsEffectSource_Impl for LumaDarkenEffect_Impl {}

impl IGraphicsEffect_Impl for LumaDarkenEffect_Impl {
    fn Name(&self) -> Result<HSTRING> {
        Ok(self.name.borrow().clone())
    }
    fn SetName(&self, name: &HSTRING) -> Result<()> {
        *self.name.borrow_mut() = name.clone();
        Ok(())
    }
}

impl IGraphicsEffectD2D1Interop_Impl for LumaDarkenEffect_Impl {
    fn GetEffectId(&self) -> Result<windows::core::GUID> {
        Ok(CLSID_D2D1ColorMatrix)
    }

    fn GetNamedPropertyMapping(
        &self,
        _name: &PCWSTR,
        _index: *mut u32,
        mapping: *mut GRAPHICS_EFFECT_PROPERTY_MAPPING,
    ) -> Result<()> {
        if !mapping.is_null() {
            unsafe { *mapping = GRAPHICS_EFFECT_PROPERTY_MAPPING_DIRECT };
        }
        Err(E_INVALIDARG.into())
    }

    fn GetPropertyCount(&self) -> Result<u32> {
        Ok(3)
    }

    fn GetProperty(&self, index: u32) -> Result<IPropertyValue> {
        let val = match index {
            i if i == D2D1_COLORMATRIX_PROP_COLOR_MATRIX.0 as u32 => {
                PropertyValue::CreateSingleArray(&self.matrix)?
            }
            i if i == D2D1_COLORMATRIX_PROP_ALPHA_MODE.0 as u32 => {
                PropertyValue::CreateUInt32(D2D1_COLORMATRIX_ALPHA_MODE_PREMULTIPLIED.0 as u32)?
            }
            i if i == D2D1_COLORMATRIX_PROP_CLAMP_OUTPUT.0 as u32 => {
                PropertyValue::CreateBoolean(true)?
            }
            _ => return Err(E_INVALIDARG.into()),
        };
        val.cast()
    }

    fn GetSource(&self, index: u32) -> Result<IGraphicsEffectSource> {
        if index == 0 {
            Ok(self.source.clone())
        } else {
            Err(E_INVALIDARG.into())
        }
    }

    fn GetSourceCount(&self) -> Result<u32> {
        Ok(1)
    }
}

#[implement(IGraphicsEffect, IGraphicsEffectSource, IGraphicsEffectD2D1Interop)]
struct CompositeEffect {
    name: RefCell<HSTRING>,
    src0: IGraphicsEffectSource,
    src1: IGraphicsEffectSource,
}

impl IGraphicsEffectSource_Impl for CompositeEffect_Impl {}

impl IGraphicsEffect_Impl for CompositeEffect_Impl {
    fn Name(&self) -> Result<HSTRING> {
        Ok(self.name.borrow().clone())
    }
    fn SetName(&self, name: &HSTRING) -> Result<()> {
        *self.name.borrow_mut() = name.clone();
        Ok(())
    }
}

impl IGraphicsEffectD2D1Interop_Impl for CompositeEffect_Impl {
    fn GetEffectId(&self) -> Result<windows::core::GUID> {
        Ok(CLSID_D2D1Composite)
    }

    fn GetNamedPropertyMapping(
        &self,
        _name: &PCWSTR,
        _index: *mut u32,
        mapping: *mut GRAPHICS_EFFECT_PROPERTY_MAPPING,
    ) -> Result<()> {
        if !mapping.is_null() {
            unsafe { *mapping = GRAPHICS_EFFECT_PROPERTY_MAPPING_DIRECT };
        }
        Err(E_INVALIDARG.into())
    }

    fn GetPropertyCount(&self) -> Result<u32> {
        Ok(1)
    }

    fn GetProperty(&self, index: u32) -> Result<IPropertyValue> {
        if index == D2D1_COMPOSITE_PROP_MODE.0 as u32 {
            PropertyValue::CreateUInt32(D2D1_COMPOSITE_MODE_SOURCE_OVER.0 as u32)?.cast()
        } else {
            Err(E_INVALIDARG.into())
        }
    }

    fn GetSource(&self, index: u32) -> Result<IGraphicsEffectSource> {
        match index {
            0 => Ok(self.src0.clone()),
            1 => Ok(self.src1.clone()),
            _ => Err(E_INVALIDARG.into()),
        }
    }

    fn GetSourceCount(&self) -> Result<u32> {
        Ok(2)
    }
}

#[implement(IGeometrySource2D, IGeometrySource2DInterop)]
struct PathSource(ID2D1PathGeometry);

impl IGeometrySource2D_Impl for PathSource_Impl {}

impl IGeometrySource2DInterop_Impl for PathSource_Impl {
    fn GetGeometry(&self) -> Result<ID2D1Geometry> {
        self.0.cast()
    }

    fn TryGetGeometryUsingFactory(&self, _factory: Ref<'_, ID2D1Factory>) -> Result<ID2D1Geometry> {
        self.0.cast()
    }
}

pub(super) struct GlassSurface {
    compositor: Compositor,
    _target: DesktopWindowTarget,
    root: ContainerVisual,
    backdrop: SpriteVisual,
    content: SpriteVisual,
    active: bool,
}

impl GlassSurface {
    pub(super) fn new(hwnd: HWND, swap: &IDXGISwapChain1, w: i32, h: i32, shift: i32) -> Result<Self> {
        ensure_dispatcher()?;
        let enable: u32 = 1;
        let _ = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_HOSTBACKDROPBRUSH,
                &enable as *const u32 as *const _,
                std::mem::size_of::<u32>() as u32,
            )
        };
        let compositor = Compositor::new()?;
        let desktop: ICompositorDesktopInterop = compositor.cast()?;
        let target = unsafe { desktop.CreateDesktopWindowTarget(hwnd, true)? };
        let interop: ICompositorInterop = compositor.cast()?;
        let comp_surface = unsafe { interop.CreateCompositionSurfaceForSwapChain(swap)? };

        let root = compositor.CreateContainerVisual()?;
        let size = Vector2 { X: w as f32, Y: h as f32 };
        root.SetSize(size)?;
        root.SetOffset(Vector3 { X: -(shift as f32), Y: 0.0, Z: 0.0 })?;

        let backdrop = compositor.CreateSpriteVisual()?;
        backdrop.SetSize(size)?;
        backdrop.SetOpacity(0.0)?;
        let brush = Self::make_blur_brush(&compositor)?;
        backdrop.SetBrush(&brush)?;

        let content = compositor.CreateSpriteVisual()?;
        content.SetSize(size)?;
        let surface_brush = compositor.CreateSurfaceBrushWithSurface(&comp_surface)?;
        surface_brush.SetStretch(CompositionStretch::None)?;
        content.SetBrush(&surface_brush)?;

        let children = root.Children()?;
        children.InsertAtTop(&backdrop)?;
        children.InsertAtTop(&content)?;
        target.SetRoot(&root)?;

        Ok(Self { compositor, _target: target, root, backdrop, content, active: false })
    }

    fn make_blur_brush(compositor: &Compositor) -> Result<CompositionEffectBrush> {
        let backdrop_param = || -> Result<IGraphicsEffectSource> {
            CompositionEffectSourceParameter::Create(&HSTRING::from("Backdrop"))?.cast()
        };
        let blur: IGraphicsEffectSource = GaussianBlurEffect {
            name: RefCell::new(HSTRING::from("Blur")),
            blur: GLASS_BLUR,
            source: backdrop_param()?,
        }
        .into();
        let vibrant: IGraphicsEffectSource = SaturationEffect {
            name: RefCell::new(HSTRING::from("Saturation")),
            saturation: GLASS_SATURATION,
            source: blur.clone(),
        }
        .into();
        let luma_blur: IGraphicsEffectSource = GaussianBlurEffect {
            name: RefCell::new(HSTRING::from("LumaBlur")),
            blur: GLASS_BLUR,
            source: backdrop_param()?,
        }
        .into();
        let luma: IGraphicsEffectSource =
            LumaDarkenEffect::new(AUTO_CONTRAST_SLOPE, AUTO_CONTRAST_THRESHOLD, luma_blur).into();
        let adaptive: IGraphicsEffect = CompositeEffect {
            name: RefCell::new(HSTRING::from("Adaptive")),
            src0: vibrant.clone(),
            src1: luma,
        }
        .into();
        let factory = compositor
            .CreateEffectFactory(&adaptive)
            .or_else(|_| compositor.CreateEffectFactory(&vibrant.cast::<IGraphicsEffect>()?))
            .or_else(|_| compositor.CreateEffectFactory(&blur.cast::<IGraphicsEffect>()?))?;
        let brush = factory.CreateBrush()?;
        let host = compositor.CreateHostBackdropBrush()?;
        brush.SetSourceParameter(&HSTRING::from("Backdrop"), &host)?;
        Ok(brush)
    }

    pub(super) fn resize(&self, w: i32, h: i32) -> Result<()> {
        let size = Vector2 { X: w as f32, Y: h as f32 };
        self.root.SetSize(size)?;
        self.backdrop.SetSize(size)?;
        self.content.SetSize(size)?;
        Ok(())
    }

    pub(super) fn set_shift(&self, shift: i32) -> Result<()> {
        self.root.SetOffset(Vector3 { X: -(shift as f32), Y: 0.0, Z: 0.0 })
    }

    pub(super) fn sync(&mut self, shape: &ID2D1PathGeometry, gy: f32, opacity: f32) -> Result<()> {
        if opacity <= 0.001 {
            if self.active {
                self.backdrop.SetOpacity(0.0)?;
                self.backdrop.SetClip(None)?;
                self.active = false;
            }
            return Ok(());
        }
        let source: IGeometrySource2D = PathSource(shape.clone()).into();
        let path = CompositionPath::Create(&source)?;
        let geom = self.compositor.CreatePathGeometryWithPath(&path)?;
        let clip = self.compositor.CreateGeometricClipWithGeometry(&geom)?;
        clip.SetOffset(Vector2 { X: 0.0, Y: gy })?;
        self.backdrop.SetClip(&clip)?;
        self.backdrop.SetOpacity(opacity.clamp(0.0, 1.0))?;
        self.active = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_glass_only_softens_expanded_pages_and_keeps_pill_and_alert_solid() {
        assert_eq!(page_bg_alpha(false, 1.0, true), 1.0, "alerts stay solid black");
        assert_eq!(page_bg_alpha(true, 0.0, true), 1.0, "closed pill stays solid black");
        assert_eq!(page_bg_alpha(true, 1.0, false), 1.0, "fallback without glass stays solid black");
        assert!((page_bg_alpha(true, 1.0, true) - PAGE_GLASS_ALPHA).abs() < 1e-5, "open page uses frosted alpha");
        let mid = page_bg_alpha(true, 0.5, true);
        assert!(mid > PAGE_GLASS_ALPHA && mid < 1.0, "opening animates smoothly: {mid}");
    }

    #[test]
    fn vibrant_text_lifts_muted_grey_on_glass_but_keeps_dark_and_color_intact() {
        let muted = D2D1_COLOR_F { r: 0.4, g: 0.4, b: 0.4, a: 1.0 };
        assert_eq!(vibrant_text(muted, false).r, 0.4, "unchanged off glass");
        let lifted = vibrant_text(muted, true);
        assert_eq!((lifted.r, lifted.g, lifted.b), (1.0, 1.0, 1.0));
        assert!(lifted.a > 0.6 && lifted.a < 0.7);
        let dark = D2D1_COLOR_F { r: 0.05, g: 0.05, b: 0.05, a: 1.0 };
        assert_eq!(vibrant_text(dark, true).r, 0.05, "dark text on white pill stays dark");
        let blue = D2D1_COLOR_F { r: 0.3, g: 0.62, b: 1.0, a: 1.0 };
        assert_eq!(vibrant_text(blue, true).r, 0.3, "accent color stays intact");
    }
}
