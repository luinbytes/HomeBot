use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeMode {
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub canvas: Color32,
    pub sidebar: Color32,
    pub surface: Color32,
    pub surface_hover: Color32,
    pub surface_selected: Color32,
    pub border: Color32,
    pub text_primary: Color32,
    pub text_secondary: Color32,
    pub text_tertiary: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub danger: Color32,
    pub overlay: Color32,
    pub bot_nova: Color32,
    pub bot_patch: Color32,
    pub bot_scout: Color32,
    pub avatar_foreground: Color32,
    pub transparent: Color32,
}

#[derive(Clone, Copy, Debug)]
pub struct Typography {
    pub display: f32,
    pub title: f32,
    pub heading: f32,
    pub body: f32,
    pub body_compact: f32,
    pub caption: f32,
    pub micro: f32,
    pub line_height: f32,
}

impl Typography {
    #[must_use]
    pub fn font(self, size: f32) -> FontId {
        FontId::new(size, FontFamily::Proportional)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Spacing {
    pub xxs: f32,
    pub xs: f32,
    pub sm: f32,
    pub md: f32,
    pub lg: f32,
    pub xl: f32,
    pub xxl: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Insets {
    pub sm: i8,
    pub md: i8,
    pub lg: i8,
    pub xl: i8,
}

#[derive(Clone, Copy, Debug)]
pub struct Radii {
    pub xs: u8,
    pub sm: u8,
    pub md: u8,
    pub lg: u8,
    pub composer: u8,
    pub pill: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub reference_width: f32,
    pub sidebar_width: f32,
    pub content_max_width: f32,
    pub titlebar_height: f32,
    pub roster_row_height: f32,
    pub avatar_size: f32,
    pub avatar_small: f32,
    pub bot_tile_height: f32,
    pub sidebar_search_height: f32,
    pub sidebar_action_height: f32,
    pub assistant_message_max_width: f32,
    pub user_message_max_width: f32,
    pub composer_editor_height: f32,
    pub composer_min_height: f32,
    pub composer_max_width: f32,
    pub empty_state_top_padding: f32,
    pub activity_icon_size: f32,
    pub activity_detail_max_height: f32,
    pub unread_dot: f32,
    pub hairline: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Motion {
    pub instant_ms: u16,
    pub quick_ms: u16,
    pub standard_ms: u16,
    pub deliberate_ms: u16,
}

#[derive(Clone, Copy, Debug)]
pub struct HomeBotTheme {
    pub mode: ThemeMode,
    pub palette: Palette,
    pub typography: Typography,
    pub spacing: Spacing,
    pub insets: Insets,
    pub radii: Radii,
    pub layout: Layout,
    pub motion: Motion,
    pub panel_shadow: Shadow,
    pub popup_shadow: Shadow,
}

impl HomeBotTheme {
    #[must_use]
    pub fn light() -> Self {
        Self {
            mode: ThemeMode::Light,
            palette: Palette {
                canvas: Color32::from_rgb(252, 252, 252),
                sidebar: Color32::from_rgb(247, 247, 247),
                surface: Color32::from_rgb(252, 252, 252),
                surface_hover: Color32::from_rgba_unmultiplied(119, 119, 119, 23),
                surface_selected: Color32::from_rgba_unmultiplied(119, 119, 119, 43),
                border: Color32::from_rgba_unmultiplied(20, 20, 20, 38),
                text_primary: Color32::from_rgb(20, 20, 20),
                text_secondary: Color32::from_rgba_unmultiplied(20, 20, 20, 153),
                text_tertiary: Color32::from_rgba_unmultiplied(20, 20, 20, 102),
                accent: Color32::from_rgb(0, 109, 204),
                accent_soft: Color32::from_rgba_unmultiplied(0, 109, 204, 44),
                success: Color32::from_rgb(0, 201, 114),
                warning: Color32::from_rgb(255, 152, 0),
                danger: Color32::from_rgb(255, 38, 60),
                overlay: Color32::from_black_alpha(92),
                bot_nova: Color32::from_rgb(109, 93, 232),
                bot_patch: Color32::from_rgb(40, 156, 112),
                bot_scout: Color32::from_rgb(222, 129, 54),
                avatar_foreground: Color32::WHITE,
                transparent: Color32::TRANSPARENT,
            },
            typography: Typography::VALUES,
            spacing: Spacing::VALUES,
            insets: Insets::VALUES,
            radii: Radii::VALUES,
            layout: Layout::VALUES,
            motion: Motion::VALUES,
            panel_shadow: Shadow {
                offset: [0, 2],
                blur: 12,
                spread: 0,
                color: Color32::from_black_alpha(28),
            },
            popup_shadow: Shadow {
                offset: [0, 8],
                blur: 28,
                spread: 0,
                color: Color32::from_black_alpha(42),
            },
        }
    }

    #[must_use]
    pub fn dark() -> Self {
        Self {
            mode: ThemeMode::Dark,
            palette: Palette {
                canvas: Color32::from_rgb(7, 7, 7),
                sidebar: Color32::from_rgb(17, 17, 17),
                surface: Color32::from_rgb(24, 24, 24),
                surface_hover: Color32::from_rgba_unmultiplied(119, 119, 119, 44),
                surface_selected: Color32::from_rgba_unmultiplied(119, 119, 119, 82),
                border: Color32::from_rgba_unmultiplied(252, 252, 252, 38),
                text_primary: Color32::from_rgb(252, 252, 252),
                text_secondary: Color32::from_rgba_unmultiplied(252, 252, 252, 153),
                text_tertiary: Color32::from_rgba_unmultiplied(252, 252, 252, 102),
                accent: Color32::from_rgb(16, 132, 254),
                accent_soft: Color32::from_rgba_unmultiplied(16, 132, 254, 44),
                success: Color32::from_rgb(0, 201, 114),
                warning: Color32::from_rgb(255, 152, 0),
                danger: Color32::from_rgb(255, 38, 60),
                overlay: Color32::from_black_alpha(148),
                bot_nova: Color32::from_rgb(137, 124, 255),
                bot_patch: Color32::from_rgb(69, 189, 139),
                bot_scout: Color32::from_rgb(241, 153, 80),
                avatar_foreground: Color32::WHITE,
                transparent: Color32::TRANSPARENT,
            },
            typography: Typography::VALUES,
            spacing: Spacing::VALUES,
            insets: Insets::VALUES,
            radii: Radii::VALUES,
            layout: Layout::VALUES,
            motion: Motion::VALUES,
            panel_shadow: Shadow {
                offset: [0, 2],
                blur: 14,
                spread: 0,
                color: Color32::from_black_alpha(92),
            },
            popup_shadow: Shadow {
                offset: [0, 8],
                blur: 30,
                spread: 0,
                color: Color32::from_black_alpha(132),
            },
        }
    }

    #[must_use]
    pub fn with_text_scale(mut self, scale: f32) -> Self {
        let scale = scale.clamp(0.8, 2.0);
        self.typography.display *= scale;
        self.typography.title *= scale;
        self.typography.heading *= scale;
        self.typography.body *= scale;
        self.typography.body_compact *= scale;
        self.typography.caption *= scale;
        self.typography.micro *= scale;
        self
    }

    #[must_use]
    pub const fn with_reduced_motion(mut self, reduced: bool) -> Self {
        if reduced {
            self.motion = Motion::REDUCED;
        }
        self
    }

    #[must_use]
    pub const fn message_assistant(self) -> Color32 {
        match self.mode {
            ThemeMode::Light => Color32::from_rgb(238, 238, 238),
            ThemeMode::Dark => Color32::from_rgb(38, 38, 38),
        }
    }

    #[must_use]
    pub const fn message_user(self) -> Color32 {
        match self.mode {
            ThemeMode::Light => Color32::from_rgb(7, 7, 7),
            ThemeMode::Dark => Color32::from_rgb(90, 90, 90),
        }
    }

    #[must_use]
    pub const fn input_fill(self) -> Color32 {
        match self.mode {
            ThemeMode::Light => Color32::from_rgb(246, 246, 246),
            ThemeMode::Dark => Color32::from_rgb(12, 12, 12),
        }
    }

    #[must_use]
    pub const fn on_accent(self) -> Color32 {
        match self.mode {
            ThemeMode::Light => Color32::WHITE,
            ThemeMode::Dark => Color32::from_rgb(5, 5, 5),
        }
    }

    #[must_use]
    pub const fn danger_fill(self) -> Color32 {
        match self.mode {
            ThemeMode::Light => Color32::from_rgb(198, 40, 40),
            ThemeMode::Dark => Color32::from_rgb(185, 28, 28),
        }
    }

    pub fn install(self, context: &egui::Context) {
        let mut style = (*context.style()).clone();
        style.spacing.item_spacing = Vec2::splat(self.spacing.sm);
        style.spacing.button_padding = Vec2::new(self.spacing.md, self.spacing.sm);
        style.spacing.interact_size =
            Vec2::new(self.spacing.xxl, self.layout.sidebar_action_height);
        style.spacing.menu_margin = Margin::same(self.insets.sm);
        style.spacing.indent = self.spacing.lg;
        style.visuals.dark_mode = self.mode == ThemeMode::Dark;
        style.visuals.panel_fill = self.palette.canvas;
        style.visuals.window_fill = self.palette.surface;
        style.visuals.extreme_bg_color = self.input_fill();
        style.visuals.faint_bg_color = self.palette.surface_hover;
        style.visuals.window_stroke = Stroke::new(self.layout.hairline, self.palette.border);
        style.visuals.window_corner_radius = CornerRadius::same(self.radii.lg);
        style.visuals.menu_corner_radius = CornerRadius::same(self.radii.md);
        style.visuals.window_shadow = self.panel_shadow;
        style.visuals.popup_shadow = self.popup_shadow;
        style.visuals.selection.bg_fill = self.palette.accent_soft;
        style.visuals.selection.stroke = Stroke::new(self.layout.hairline, self.palette.accent);
        style.visuals.hyperlink_color = self.palette.accent;
        style.visuals.warn_fg_color = self.palette.warning;
        style.visuals.error_fg_color = self.palette.danger;
        style.visuals.widgets.noninteractive.fg_stroke.color = self.palette.text_primary;
        style.visuals.widgets.inactive.bg_fill = self.palette.surface;
        style.visuals.widgets.inactive.weak_bg_fill = self.palette.surface_hover;
        style.visuals.widgets.inactive.bg_stroke =
            Stroke::new(self.layout.hairline, self.palette.border);
        style.visuals.widgets.inactive.fg_stroke.color = self.palette.text_secondary;
        style.visuals.widgets.inactive.corner_radius = CornerRadius::same(self.radii.sm);
        style.visuals.widgets.hovered.bg_fill = self.palette.surface_hover;
        style.visuals.widgets.hovered.weak_bg_fill = self.palette.surface_selected;
        style.visuals.widgets.hovered.bg_stroke =
            Stroke::new(self.layout.hairline, self.palette.accent);
        style.visuals.widgets.hovered.fg_stroke.color = self.palette.text_primary;
        style.visuals.widgets.hovered.corner_radius = CornerRadius::same(self.radii.sm);
        style.visuals.widgets.active.bg_fill = self.palette.surface_selected;
        style.visuals.widgets.active.weak_bg_fill = self.palette.surface_selected;
        style.visuals.widgets.active.bg_stroke =
            Stroke::new(self.layout.hairline, self.palette.accent);
        style.visuals.widgets.active.fg_stroke.color = self.palette.text_primary;
        style.visuals.widgets.active.corner_radius = CornerRadius::same(self.radii.sm);
        style.animation_time = f32::from(self.motion.standard_ms) / 1_000.0;
        style.text_styles = [
            (
                TextStyle::Heading,
                self.typography.font(self.typography.display),
            ),
            (TextStyle::Body, self.typography.font(self.typography.body)),
            (
                TextStyle::Button,
                self.typography.font(self.typography.body_compact),
            ),
            (
                TextStyle::Small,
                self.typography.font(self.typography.caption),
            ),
        ]
        .into();
        context.set_style(style);
    }
}

impl Typography {
    const VALUES: Self = Self {
        display: 24.0,
        title: 20.0,
        heading: 16.0,
        body: 14.0,
        body_compact: 13.0,
        caption: 12.0,
        micro: 11.0,
        line_height: 1.42,
    };
}

impl Spacing {
    const VALUES: Self = Self {
        xxs: 2.0,
        xs: 4.0,
        sm: 8.0,
        md: 12.0,
        lg: 16.0,
        xl: 24.0,
        xxl: 32.0,
    };
}

impl Insets {
    const VALUES: Self = Self {
        sm: 8,
        md: 12,
        lg: 16,
        xl: 24,
    };
}

impl Radii {
    const VALUES: Self = Self {
        xs: 4,
        sm: 8,
        md: 12,
        lg: 18,
        composer: 16,
        pill: 255,
    };
}

impl Layout {
    pub const REFERENCE_HEIGHT: f32 = 760.0;
    pub const SIDEBAR_COLLAPSED_WIDTH: f32 = 0.0;
    pub const SIDEBAR_MIN_WIDTH: f32 = 240.0;
    pub const SIDEBAR_RATIO: f32 = 0.30;
    pub const BOT_TILE_MIN_WIDTH: f32 = 116.0;
    pub const COMPOSER_ACTION_RESERVE: f32 = 76.0;
    pub const CONTEXTUAL_ACTION_HEIGHT: f32 = 24.0;
    pub const SETTINGS_MODAL_WIDTH: f32 = 860.0;
    pub const MODAL_WIDTH: f32 = 640.0;
    pub const MODAL_COMPACT_WIDTH: f32 = 460.0;
    pub const MODAL_MAX_HEIGHT: f32 = 620.0;

    const VALUES: Self = Self {
        reference_width: 1120.0,
        sidebar_width: 280.0,
        content_max_width: 690.0,
        titlebar_height: 52.0,
        roster_row_height: 58.0,
        avatar_size: 34.0,
        avatar_small: 28.0,
        bot_tile_height: 104.0,
        sidebar_search_height: 34.0,
        sidebar_action_height: 34.0,
        assistant_message_max_width: 640.0,
        user_message_max_width: 640.0,
        composer_editor_height: 42.0,
        composer_min_height: 74.0,
        composer_max_width: 700.0,
        empty_state_top_padding: 210.0,
        activity_icon_size: 16.0,
        activity_detail_max_height: 220.0,
        unread_dot: 7.0,
        hairline: 1.0,
    };
}

impl Motion {
    const REDUCED: Self = Self {
        instant_ms: 0,
        quick_ms: 0,
        standard_ms: 0,
        deliberate_ms: 0,
    };

    const VALUES: Self = Self {
        instant_ms: 0,
        quick_ms: 120,
        standard_ms: 200,
        deliberate_ms: 260,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes_share_geometry_and_meet_basic_contrast_guards() {
        let light = HomeBotTheme::light();
        let dark = HomeBotTheme::dark();
        assert!((light.layout.reference_width - dark.layout.reference_width).abs() < f32::EPSILON);
        assert!(light.palette.text_primary.r() < light.palette.canvas.r());
        assert!(dark.palette.text_primary.r() > dark.palette.canvas.r());
        assert!(
            (light.layout.composer_max_width - (light.layout.content_max_width + 10.0)).abs()
                < f32::EPSILON
        );
        assert!(light.motion.quick_ms < light.motion.deliberate_ms);
        assert!(contrast(light.palette.accent, light.on_accent()) >= 4.5);
        assert!(contrast(dark.palette.accent, dark.on_accent()) >= 4.5);
        assert!(contrast(light.danger_fill(), Color32::WHITE) >= 4.5);
        assert!(contrast(dark.danger_fill(), Color32::WHITE) >= 4.5);
    }

    #[test]
    fn desktop_geometry_matches_the_pinned_grok_bot_reference() {
        let layout = HomeBotTheme::dark().layout;
        let exact = |actual: f32, expected: f32| {
            assert!((actual - expected).abs() < f32::EPSILON);
        };
        exact(layout.sidebar_width, 280.0);
        exact(layout.titlebar_height, 52.0);
        exact(layout.roster_row_height, 58.0);
        exact(layout.content_max_width, 690.0);
        exact(layout.assistant_message_max_width, 640.0);
        exact(layout.composer_max_width, 700.0);
        exact(layout.activity_icon_size, 16.0);
        exact(layout.activity_detail_max_height, 220.0);
    }

    #[test]
    fn text_scaling_is_clamped_and_preserves_geometry() {
        let base = HomeBotTheme::light();
        let large = base.with_text_scale(2.5);
        let small = base.with_text_scale(0.1);
        assert!((large.typography.body - base.typography.body * 2.0).abs() < f32::EPSILON);
        assert!((small.typography.body - base.typography.body * 0.8).abs() < f32::EPSILON);
        assert!((large.layout.sidebar_width - base.layout.sidebar_width).abs() < f32::EPSILON);
    }

    #[test]
    fn component_sources_do_not_define_palette_literals() {
        for source in [include_str!("components.rs"), include_str!("showcase.rs")] {
            assert!(!source.contains("Color32::from_"));
            assert!(!source.contains("Color32::WHITE"));
            assert!(!source.contains("Color32::BLACK"));
        }
    }

    fn contrast(first: Color32, second: Color32) -> f32 {
        let luminance = |color: Color32| {
            let channel = |value: u8| {
                let value = f32::from(value) / 255.0;
                if value <= 0.040_45 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
        };
        let first = luminance(first);
        let second = luminance(second);
        (first.max(second) + 0.05) / (first.min(second) + 0.05)
    }
}
