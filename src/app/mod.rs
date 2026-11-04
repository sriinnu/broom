//! The main window. `Broom` is the view; each file holds one part of it.
//! Everything here is one unit, so the files share these imports and each
//! other's items through `use super::*`.

mod api_keys;
mod broom;
mod chart;
mod cleanup;
mod lifecycle;
mod live_view;
mod modals;
mod navigation;
mod navigator;
mod panes;
mod query;
mod results;
mod scan_flow;
mod scanning;
mod settings_panel;
mod toolbar;
mod types;
mod widgets;
#[cfg(test)]
mod tests;

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, LazyLock};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use gpui::{
    AnyElement, App, AsyncApp, Bounds, ClickEvent, ClipboardItem, Context, CursorStyle, DispatchPhase, Entity, FocusHandle, Focusable,
    FontWeight, HapticFeedbackStyle, HitboxBehavior, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PathPromptOptions, Pixels, Point, Rgba, ScrollHandle, ScrollStrategy, SharedString, Stateful, Subscription, Task, TextAlign, TextRun,
    TitlebarOptions, UniformListScrollHandle, BorderStyle, Window, WindowBackgroundAppearance, WindowBounds, WindowHandle,
    WindowOptions, actions, anchored, canvas, deferred, div, point, prelude::*, px, quad, relative, rgb, size,
    uniform_list, img, Image, ImageFormat,
};

use gpui_ce_elements::editable_text::{EditableTextState, StringStorage, TextChanged, text_input};
use objc2::MainThreadMarker;
use objc2::ClassType;
use objc2::rc::Retained;
use objc2::runtime::NSObjectProtocol;
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSEvent, NSEventModifierFlags, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindowOrderingMode,
};
use palette::IntoColor;

use crate::ask::{self, CandidateGroup, Intent, ProviderError, Understanding};
use crate::classify::{self, CATEGORIES, Category};
use crate::clock;
use crate::comic;
use crate::disk;
use crate::eta;
use crate::onboarding;
use crate::findings::{self, Finding, Safety};
use crate::live;
use crate::menubar;
use crate::motion;
use crate::scan::{self, CloudRoot, Kind, Progress, Tree, Volume, format_count, format_size};
use crate::secrets;
use crate::settings::{self, Settings};
use crate::duplicates;
use crate::history;
use crate::theme::{self, colors};
use crate::trashcan;
use crate::sunburst::{self, Geometry, Hit, Segment, Target};

actions!(broom, [
    GoUp, OpenFolder, Rescan, StartOver, FocusQuery, SubmitQuery, DismissQuery,
    OpenSettings, CloseSettings, About, CloseAbout,
    SelectNext, SelectPrev, OpenSelected, ToggleStaged, CloseReview, CopyPath, CopyRelativePath,
    SortBySize, SortByName, SortByModified,
    GoBack, GoForward, ToggleRail, SelectAll
]);

pub use self::broom::*;
use self::chart::*;
pub use self::lifecycle::*;
use self::live_view::*;
#[cfg(test)]
use self::navigator::*;
#[cfg(test)]
use self::panes::*;
use self::results::*;
use self::types::*;
use self::widgets::*;
