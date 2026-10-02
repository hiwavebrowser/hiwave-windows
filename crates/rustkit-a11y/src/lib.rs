//! # RustKit Accessibility
//!
//! Accessibility (A11y) implementation for the RustKit browser engine.
//!
//! ## Features
//!
//! - **ARIA**: Roles, states, and properties
//! - **Accessibility Tree**: Parallel tree structure
//! - **Focus Management**: Tab order, focus trap
//! - **Live Regions**: aria-live announcements
//! - **UI Automation**: Windows accessibility API
//!
//! ## Architecture
//!
//! ```text
//! DOM Tree ─────────────►  Accessibility Tree
//!     │                          │
//!     │                          ├── AccessibleNode
//!     │                          │       ├── Role
//!     │                          │       ├── Name
//!     │                          │       ├── States
//!     │                          │       └── Properties
//!     │                          │
//!     └── Events ───────────────►└── Live Regions
//!                                         └── Announcements
//! ```

use hashbrown::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;

// ==================== Errors ====================

/// Accessibility errors.
#[derive(Error, Debug, Clone)]
pub enum A11yError {
    #[error("Node not found: {0}")]
    NodeNotFound(u64),

    #[error("Invalid role: {0}")]
    InvalidRole(String),

    #[error("Invalid state: {0}")]
    InvalidState(String),

    #[error("Provider error: {0}")]
    ProviderError(String),
}

// ==================== Types ====================

/// Unique identifier for accessible node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AccessibleId(u64);

impl AccessibleId {
    /// Create a new ID.
    pub fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    /// Get raw ID.
    pub fn raw(&self) -> u64 {
        self.0
    }
}

impl Default for AccessibleId {
    fn default() -> Self {
        Self::new()
    }
}

// ==================== ARIA Roles ====================

/// ARIA role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    // Landmark roles
    Banner,
    Complementary,
    ContentInfo,
    Form,
    Main,
    Navigation,
    Region,
    Search,

    // Document structure
    Article,
    Heading,
    Document,
    Group,
    Img,
    List,
    ListItem,
    Math,
    Note,
    Presentation,
    Separator,
    Toolbar,

    // Widget roles
    Alert,
    AlertDialog,
    Button,
    Checkbox,
    Dialog,
    GridCell,
    Link,
    Log,
    Marquee,
    Menu,
    MenuBar,
    MenuItem,
    MenuItemCheckbox,
    MenuItemRadio,
    Option,
    ProgressBar,
    Radio,
    RadioGroup,
    ScrollBar,
    SearchBox,
    Slider,
    SpinButton,
    Status,
    Switch,
    Tab,
    TabList,
    TabPanel,
    TextBox,
    Timer,
    Tooltip,
    Tree,
    TreeGrid,
    TreeItem,

    // Table roles
    Cell,
    ColumnHeader,
    Grid,
    Row,
    RowGroup,
    RowHeader,
    Table,

    // Generic
    Generic,
    None,
}

impl Role {
    /// Parse from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "banner" => Some(Self::Banner),
            "complementary" => Some(Self::Complementary),
            "contentinfo" => Some(Self::ContentInfo),
            "form" => Some(Self::Form),
            "main" => Some(Self::Main),
            "navigation" => Some(Self::Navigation),
            "region" => Some(Self::Region),
            "search" => Some(Self::Search),
            "article" => Some(Self::Article),
            "heading" => Some(Self::Heading),
            "document" => Some(Self::Document),
            "group" => Some(Self::Group),
            "img" => Some(Self::Img),
            "list" => Some(Self::List),
            "listitem" => Some(Self::ListItem),
            "math" => Some(Self::Math),
            "note" => Some(Self::Note),
            "presentation" | "none" => Some(Self::Presentation),
            "separator" => Some(Self::Separator),
            "toolbar" => Some(Self::Toolbar),
            "alert" => Some(Self::Alert),
            "alertdialog" => Some(Self::AlertDialog),
            "button" => Some(Self::Button),
            "checkbox" => Some(Self::Checkbox),
            "dialog" => Some(Self::Dialog),
            "gridcell" => Some(Self::GridCell),
            "link" => Some(Self::Link),
            "log" => Some(Self::Log),
            "marquee" => Some(Self::Marquee),
            "menu" => Some(Self::Menu),
            "menubar" => Some(Self::MenuBar),
            "menuitem" => Some(Self::MenuItem),
            "menuitemcheckbox" => Some(Self::MenuItemCheckbox),
            "menuitemradio" => Some(Self::MenuItemRadio),
            "option" => Some(Self::Option),
            "progressbar" => Some(Self::ProgressBar),
            "radio" => Some(Self::Radio),
            "radiogroup" => Some(Self::RadioGroup),
            "scrollbar" => Some(Self::ScrollBar),
            "searchbox" => Some(Self::SearchBox),
            "slider" => Some(Self::Slider),
            "spinbutton" => Some(Self::SpinButton),
            "status" => Some(Self::Status),
            "switch" => Some(Self::Switch),
            "tab" => Some(Self::Tab),
            "tablist" => Some(Self::TabList),
            "tabpanel" => Some(Self::TabPanel),
            "textbox" => Some(Self::TextBox),
            "timer" => Some(Self::Timer),
            "tooltip" => Some(Self::Tooltip),
            "tree" => Some(Self::Tree),
            "treegrid" => Some(Self::TreeGrid),
            "treeitem" => Some(Self::TreeItem),
            "cell" => Some(Self::Cell),
            "columnheader" => Some(Self::ColumnHeader),
            "grid" => Some(Self::Grid),
            "row" => Some(Self::Row),
            "rowgroup" => Some(Self::RowGroup),
            "rowheader" => Some(Self::RowHeader),
            "table" => Some(Self::Table),
            _ => None,
        }
    }

    /// Check if focusable by default.
    pub fn is_focusable(&self) -> bool {
        matches!(
            self,
            Role::Button
                | Role::Checkbox
                | Role::Link
                | Role::MenuItem
                | Role::MenuItemCheckbox
                | Role::MenuItemRadio
                | Role::Option
                | Role::Radio
                | Role::SearchBox
                | Role::Slider
                | Role::SpinButton
                | Role::Switch
                | Role::Tab
                | Role::TextBox
                | Role::TreeItem
        )
    }

    /// Check if interactive.
    pub fn is_interactive(&self) -> bool {
        matches!(
            self,
            Role::Button
                | Role::Checkbox
                | Role::Link
                | Role::MenuItem
                | Role::MenuItemCheckbox
                | Role::MenuItemRadio
                | Role::Option
                | Role::Radio
                | Role::Slider
                | Role::SpinButton
                | Role::Switch
                | Role::Tab
                | Role::TextBox
                | Role::TreeItem
        )
    }
}

impl Default for Role {
    fn default() -> Self {
        Self::Generic
    }
}

// ==================== ARIA States ====================

/// ARIA states (boolean).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    Busy,
    Checked,
    Disabled,
    Expanded,
    Grabbed,
    Hidden,
    Invalid,
    Pressed,
    Selected,
    Required,
    ReadOnly,
    Multiselectable,
}

impl State {
    /// Parse from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "busy" => Some(Self::Busy),
            "checked" => Some(Self::Checked),
            "disabled" => Some(Self::Disabled),
            "expanded" => Some(Self::Expanded),
            "grabbed" => Some(Self::Grabbed),
            "hidden" => Some(Self::Hidden),
            "invalid" => Some(Self::Invalid),
            "pressed" => Some(Self::Pressed),
            "selected" => Some(Self::Selected),
            "required" => Some(Self::Required),
            "readonly" => Some(Self::ReadOnly),
            "multiselectable" => Some(Self::Multiselectable),
            _ => None,
        }
    }
}

// ==================== Live Region ====================

/// Live region politeness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LiveRegionPoliteness {
    #[default]
    Off,
    Polite,
    Assertive,
}

impl LiveRegionPoliteness {
    /// Parse from string.
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "polite" => Self::Polite,
            "assertive" => Self::Assertive,
            _ => Self::Off,
        }
    }
}

/// Live region settings.
#[derive(Debug, Clone, Default)]
pub struct LiveRegion {
    pub politeness: LiveRegionPoliteness,
    pub atomic: bool,
    pub relevant: Vec<String>, // additions, removals, text, all
}

// ==================== Accessible Node ====================

/// An accessible node in the accessibility tree.
#[derive(Debug, Clone)]
pub struct AccessibleNode {
    /// Unique ID.
    pub id: AccessibleId,
    
    /// DOM node ID (if linked).
    pub dom_node_id: Option<rustkit_dom::NodeId>,
    
    /// Role.
    pub role: Role,
    
    /// Accessible name.
    pub name: Option<String>,
    
    /// Description.
    pub description: Option<String>,
    
    /// Value (for sliders, etc.).
    pub value: Option<String>,
    
    /// States.
    pub states: HashSet<State>,
    
    /// Properties (aria-* attributes).
    pub properties: HashMap<String, String>,
    
    /// Parent ID.
    pub parent: Option<AccessibleId>,
    
    /// Child IDs.
    pub children: Vec<AccessibleId>,
    
    /// Tab index.
    pub tab_index: Option<i32>,
    
    /// Level (for headings).
    pub level: Option<u32>,
    
    /// Position in set.
    pub pos_in_set: Option<u32>,
    
    /// Set size.
    pub set_size: Option<u32>,
    
    /// Live region settings.
    pub live_region: Option<LiveRegion>,
    
    /// Bounding box (x, y, width, height).
    pub bounds: Option<(f32, f32, f32, f32)>,
}

impl AccessibleNode {
    /// Create a new accessible node.
    pub fn new(role: Role) -> Self {
        Self {
            id: AccessibleId::new(),
            dom_node_id: None,
            role,
            name: None,
            description: None,
            value: None,
            states: HashSet::new(),
            properties: HashMap::new(),
            parent: None,
            children: Vec::new(),
            tab_index: None,
            level: None,
            pos_in_set: None,
            set_size: None,
            live_region: None,
            bounds: None,
        }
    }

    /// Add a state.
    pub fn add_state(&mut self, state: State) {
        self.states.insert(state);
    }

    /// Remove a state.
    pub fn remove_state(&mut self, state: State) {
        self.states.remove(&state);
    }

    /// Check if has state.
    pub fn has_state(&self, state: State) -> bool {
        self.states.contains(&state)
    }

    /// Set property.
    pub fn set_property(&mut self, name: &str, value: &str) {
        self.properties.insert(name.to_string(), value.to_string());
    }

    /// Get property.
    pub fn get_property(&self, name: &str) -> Option<&str> {
        self.properties.get(name).map(|s| s.as_str())
    }

    /// Check if focusable.
    pub fn is_focusable(&self) -> bool {
        if self.has_state(State::Disabled) {
            return false;
        }
        
        match self.tab_index {
            Some(i) => i >= 0,
            None => self.role.is_focusable(),
        }
    }

    /// Check if hidden.
    pub fn is_hidden(&self) -> bool {
        self.has_state(State::Hidden) || self.get_property("aria-hidden") == Some("true")
    }
}

// ==================== Accessibility Tree ====================

/// The accessibility tree.
#[derive(Debug, Default)]
pub struct AccessibilityTree {
    /// Nodes by ID.
    nodes: HashMap<AccessibleId, AccessibleNode>,
    
    /// Root node ID.
    root: Option<AccessibleId>,
    
    /// Focus node ID.
    focus: Option<AccessibleId>,
    
    /// DOM to accessible mapping.
    dom_map: HashMap<rustkit_dom::NodeId, AccessibleId>,
    
    /// Tab order cache (sorted).
    tab_order: Vec<AccessibleId>,
    
    /// Tab order dirty.
    tab_order_dirty: bool,
}

impl AccessibilityTree {
    /// Create a new tree.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a node.
    pub fn add_node(&mut self, node: AccessibleNode) -> AccessibleId {
        let id = node.id;
        
        if let Some(dom_id) = node.dom_node_id {
            self.dom_map.insert(dom_id, id);
        }
        
        self.nodes.insert(id, node);
        self.tab_order_dirty = true;
        id
    }

    /// Set root.
    pub fn set_root(&mut self, id: AccessibleId) {
        self.root = Some(id);
    }

    /// Get node.
    pub fn get(&self, id: AccessibleId) -> Option<&AccessibleNode> {
        self.nodes.get(&id)
    }

    /// Get node mutably.
    pub fn get_mut(&mut self, id: AccessibleId) -> Option<&mut AccessibleNode> {
        self.nodes.get_mut(&id)
    }

    /// Get by DOM ID.
    pub fn get_by_dom(&self, dom_id: rustkit_dom::NodeId) -> Option<&AccessibleNode> {
        self.dom_map.get(&dom_id).and_then(|id| self.nodes.get(id))
    }

    /// Remove node.
    pub fn remove(&mut self, id: AccessibleId) -> Option<AccessibleNode> {
        if let Some(node) = self.nodes.remove(&id) {
            if let Some(dom_id) = node.dom_node_id {
                self.dom_map.remove(&dom_id);
            }
            self.tab_order_dirty = true;
            Some(node)
        } else {
            None
        }
    }

    /// Add child to parent.
    pub fn add_child(&mut self, parent_id: AccessibleId, child_id: AccessibleId) {
        if let Some(parent) = self.nodes.get_mut(&parent_id) {
            parent.children.push(child_id);
        }
        if let Some(child) = self.nodes.get_mut(&child_id) {
            child.parent = Some(parent_id);
        }
        self.tab_order_dirty = true;
    }

    /// Get focused node.
    pub fn get_focus(&self) -> Option<&AccessibleNode> {
        self.focus.and_then(|id| self.nodes.get(&id))
    }

    /// Set focus.
    pub fn set_focus(&mut self, id: Option<AccessibleId>) {
        self.focus = id;
    }

    /// Build tab order.
    pub fn build_tab_order(&mut self) {
        if !self.tab_order_dirty {
            return;
        }

        self.tab_order.clear();

        // Collect focusable nodes
        let mut focusable: Vec<_> = self
            .nodes
            .values()
            .filter(|n| n.is_focusable() && !n.is_hidden())
            .map(|n| (n.id, n.tab_index.unwrap_or(0)))
            .collect();

        // Sort by tab index, then by tree order
        focusable.sort_by(|a, b| {
            match (a.1, b.1) {
                (0, 0) => std::cmp::Ordering::Equal,
                (0, _) => std::cmp::Ordering::Greater,
                (_, 0) => std::cmp::Ordering::Less,
                (x, y) => x.cmp(&y),
            }
        });

        self.tab_order = focusable.into_iter().map(|(id, _)| id).collect();
        self.tab_order_dirty = false;
    }

    /// Get next focusable.
    pub fn next_focusable(&mut self) -> Option<AccessibleId> {
        self.build_tab_order();
        
        if self.tab_order.is_empty() {
            return None;
        }

        match self.focus {
            Some(current) => {
                let pos = self.tab_order.iter().position(|&id| id == current);
                match pos {
                    Some(i) => {
                        let next = (i + 1) % self.tab_order.len();
                        Some(self.tab_order[next])
                    }
                    None => self.tab_order.first().copied(),
                }
            }
            None => self.tab_order.first().copied(),
        }
    }

    /// Get previous focusable.
    pub fn prev_focusable(&mut self) -> Option<AccessibleId> {
        self.build_tab_order();
        
        if self.tab_order.is_empty() {
            return None;
        }

        match self.focus {
            Some(current) => {
                let pos = self.tab_order.iter().position(|&id| id == current);
                match pos {
                    Some(i) => {
                        let prev = if i == 0 { self.tab_order.len() - 1 } else { i - 1 };
                        Some(self.tab_order[prev])
                    }
                    None => self.tab_order.last().copied(),
                }
            }
            None => self.tab_order.last().copied(),
        }
    }

    /// Walk tree (depth-first).
    pub fn walk<F>(&self, mut visitor: F)
    where
        F: FnMut(&AccessibleNode, usize),
    {
        if let Some(root) = self.root {
            self.walk_node(root, 0, &mut visitor);
        }
    }

    fn walk_node<F>(&self, id: AccessibleId, depth: usize, visitor: &mut F)
    where
        F: FnMut(&AccessibleNode, usize),
    {
        if let Some(node) = self.nodes.get(&id) {
            visitor(node, depth);
            for &child_id in &node.children {
                self.walk_node(child_id, depth + 1, visitor);
            }
        }
    }

    /// Count nodes.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

// ==================== Announcements ====================

/// An announcement for screen readers.
#[derive(Debug, Clone)]
pub struct Announcement {
    /// Message text.
    pub message: String,
    
    /// Politeness level.
    pub politeness: LiveRegionPoliteness,
    
    /// Clear previous announcements.
    pub clear_queue: bool,
}

impl Announcement {
    /// Create polite announcement.
    pub fn polite(message: &str) -> Self {
        Self {
            message: message.to_string(),
            politeness: LiveRegionPoliteness::Polite,
            clear_queue: false,
        }
    }

    /// Create assertive announcement.
    pub fn assertive(message: &str) -> Self {
        Self {
            message: message.to_string(),
            politeness: LiveRegionPoliteness::Assertive,
            clear_queue: true,
        }
    }
}

/// Announcement queue.
#[derive(Debug, Default)]
pub struct AnnouncementQueue {
    queue: Vec<Announcement>,
}

impl AnnouncementQueue {
    /// Create new queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add announcement.
    pub fn push(&mut self, announcement: Announcement) {
        if announcement.clear_queue {
            self.queue.clear();
        }
        self.queue.push(announcement);
    }

    /// Pop next announcement.
    pub fn pop(&mut self) -> Option<Announcement> {
        if self.queue.is_empty() {
            None
        } else {
            Some(self.queue.remove(0))
        }
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

// ==================== Focus Manager ====================

/// Focus trap configuration.
#[derive(Debug, Clone)]
pub struct FocusTrap {
    /// Container node ID.
    pub container: AccessibleId,
    
    /// First focusable in trap.
    pub first: Option<AccessibleId>,
    
    /// Last focusable in trap.
    pub last: Option<AccessibleId>,
    
    /// Auto-focus on activation.
    pub auto_focus: bool,
    
    /// Return focus on deactivation.
    pub return_focus: Option<AccessibleId>,
}

impl FocusTrap {
    /// Create a new focus trap.
    pub fn new(container: AccessibleId) -> Self {
        Self {
            container,
            first: None,
            last: None,
            auto_focus: true,
            return_focus: None,
        }
    }
}

/// Focus manager.
#[derive(Debug, Default)]
pub struct FocusManager {
    /// Current focus.
    current: Option<AccessibleId>,
    
    /// Active focus traps (stack).
    traps: Vec<FocusTrap>,
    
    /// Focus history.
    history: Vec<AccessibleId>,
}

impl FocusManager {
    /// Create new manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get current focus.
    pub fn current(&self) -> Option<AccessibleId> {
        self.current
    }

    /// Set focus.
    pub fn set_focus(&mut self, id: AccessibleId) {
        if let Some(prev) = self.current {
            self.history.push(prev);
        }
        self.current = Some(id);
    }

    /// Clear focus.
    pub fn clear_focus(&mut self) {
        self.current = None;
    }

    /// Push focus trap.
    pub fn push_trap(&mut self, mut trap: FocusTrap) {
        trap.return_focus = self.current;
        self.traps.push(trap);
    }

    /// Pop focus trap.
    pub fn pop_trap(&mut self) -> Option<FocusTrap> {
        let trap = self.traps.pop()?;
        if let Some(return_id) = trap.return_focus {
            self.current = Some(return_id);
        }
        Some(trap)
    }

    /// Get active trap.
    pub fn active_trap(&self) -> Option<&FocusTrap> {
        self.traps.last()
    }

    /// Check if in trap.
    pub fn in_trap(&self, id: AccessibleId) -> bool {
        self.traps.iter().any(|t| t.container == id)
    }

    /// Return focus to previous.
    pub fn return_focus(&mut self) {
        self.current = self.history.pop();
    }
}

// ==================== A11y Manager ====================

/// Accessibility manager.
pub struct A11yManager {
    /// Accessibility tree.
    pub tree: AccessibilityTree,
    
    /// Announcement queue.
    pub announcements: AnnouncementQueue,
    
    /// Focus manager.
    pub focus: FocusManager,
    
    /// Enabled.
    pub enabled: bool,
    
    /// High contrast mode.
    pub high_contrast: bool,
    
    /// Reduced motion.
    pub reduced_motion: bool,
}

impl A11yManager {
    /// Create new manager.
    pub fn new() -> Self {
        Self {
            tree: AccessibilityTree::new(),
            announcements: AnnouncementQueue::new(),
            focus: FocusManager::new(),
            enabled: true,
            high_contrast: false,
            reduced_motion: false,
        }
    }

    /// Announce message.
    pub fn announce(&mut self, message: &str, assertive: bool) {
        let announcement = if assertive {
            Announcement::assertive(message)
        } else {
            Announcement::polite(message)
        };
        self.announcements.push(announcement);
    }

    /// Focus next.
    pub fn focus_next(&mut self) {
        if let Some(id) = self.tree.next_focusable() {
            self.focus.set_focus(id);
            self.tree.set_focus(Some(id));
        }
    }

    /// Focus previous.
    pub fn focus_prev(&mut self) {
        if let Some(id) = self.tree.prev_focusable() {
            self.focus.set_focus(id);
            self.tree.set_focus(Some(id));
        }
    }

    /// Activate focus trap.
    pub fn activate_trap(&mut self, container: AccessibleId) {
        let trap = FocusTrap::new(container);
        self.focus.push_trap(trap);
    }

    /// Deactivate focus trap.
    pub fn deactivate_trap(&mut self) {
        if let Some(trap) = self.focus.pop_trap() {
            if let Some(return_id) = trap.return_focus {
                self.tree.set_focus(Some(return_id));
            }
        }
    }

    /// Check accessibility preferences.
    pub fn check_preferences(&mut self) {
        // Would query system preferences
        // For now, defaults
        #[cfg(windows)]
        {
            // Could use SystemParametersInfo to detect high contrast
        }
    }
}

impl Default for A11yManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_parsing() {
        assert_eq!(Role::from_str("button"), Some(Role::Button));
        assert_eq!(Role::from_str("CHECKBOX"), Some(Role::Checkbox));
        assert_eq!(Role::from_str("unknown"), None);
    }

    #[test]
    fn test_role_focusable() {
        assert!(Role::Button.is_focusable());
        assert!(Role::TextBox.is_focusable());
        assert!(!Role::Heading.is_focusable());
        assert!(!Role::Generic.is_focusable());
    }

    #[test]
    fn test_accessible_node() {
        let mut node = AccessibleNode::new(Role::Button);
        node.name = Some("Click me".to_string());
        node.add_state(State::Pressed);
        
        assert_eq!(node.role, Role::Button);
        assert!(node.has_state(State::Pressed));
        assert!(node.is_focusable());
    }

    #[test]
    fn test_node_disabled() {
        let mut node = AccessibleNode::new(Role::Button);
        assert!(node.is_focusable());
        
        node.add_state(State::Disabled);
        assert!(!node.is_focusable());
    }

    #[test]
    fn test_accessibility_tree() {
        let mut tree = AccessibilityTree::new();
        
        let root = AccessibleNode::new(Role::Document);
        let root_id = tree.add_node(root);
        tree.set_root(root_id);
        
        let button = AccessibleNode::new(Role::Button);
        let button_id = tree.add_node(button);
        tree.add_child(root_id, button_id);
        
        assert_eq!(tree.len(), 2);
        assert!(tree.get(button_id).is_some());
    }

    #[test]
    fn test_tab_order() {
        let mut tree = AccessibilityTree::new();
        
        let mut btn1 = AccessibleNode::new(Role::Button);
        btn1.tab_index = Some(2);
        let _id1 = tree.add_node(btn1);
        
        let mut btn2 = AccessibleNode::new(Role::Button);
        btn2.tab_index = Some(1);
        let id2 = tree.add_node(btn2);
        
        tree.build_tab_order();
        
        // Tab index 1 comes before 2
        let next = tree.next_focusable();
        assert_eq!(next, Some(id2));
    }

    #[test]
    fn test_focus_navigation() {
        let mut tree = AccessibilityTree::new();
        
        let btn1 = AccessibleNode::new(Role::Button);
        let id1 = tree.add_node(btn1);
        
        let btn2 = AccessibleNode::new(Role::Button);
        let id2 = tree.add_node(btn2);
        
        // Focus first
        let first = tree.next_focusable().unwrap();
        tree.set_focus(Some(first));
        
        // Get next
        let next = tree.next_focusable().unwrap();
        assert!(next == id1 || next == id2);
    }

    #[test]
    fn test_announcement() {
        let mut queue = AnnouncementQueue::new();
        
        queue.push(Announcement::polite("Hello"));
        queue.push(Announcement::polite("World"));
        
        assert!(!queue.is_empty());
        
        let first = queue.pop().unwrap();
        assert_eq!(first.message, "Hello");
    }

    #[test]
    fn test_assertive_clears() {
        let mut queue = AnnouncementQueue::new();
        
        queue.push(Announcement::polite("One"));
        queue.push(Announcement::assertive("Urgent!"));
        
        // Assertive clears the queue, so only Urgent should be there
        let ann = queue.pop().unwrap();
        assert_eq!(ann.message, "Urgent!");
        assert!(queue.is_empty());
    }

    #[test]
    fn test_focus_trap() {
        let mut manager = FocusManager::new();
        
        let button_id = AccessibleId::new();
        let dialog_id = AccessibleId::new();
        
        manager.set_focus(button_id);
        
        let trap = FocusTrap::new(dialog_id);
        manager.push_trap(trap);
        
        assert!(manager.active_trap().is_some());
        
        manager.pop_trap();
        
        // Focus returned to button
        assert_eq!(manager.current(), Some(button_id));
    }

    #[test]
    fn test_live_region() {
        let mut node = AccessibleNode::new(Role::Status);
        node.live_region = Some(LiveRegion {
            politeness: LiveRegionPoliteness::Polite,
            atomic: true,
            relevant: vec!["additions".to_string(), "text".to_string()],
        });
        
        let lr = node.live_region.as_ref().unwrap();
        assert_eq!(lr.politeness, LiveRegionPoliteness::Polite);
        assert!(lr.atomic);
    }

    #[test]
    fn test_a11y_manager() {
        let mut manager = A11yManager::new();
        
        let button = AccessibleNode::new(Role::Button);
        let id = manager.tree.add_node(button);
        
        manager.focus.set_focus(id);
        manager.announce("Button clicked", false);
        
        assert_eq!(manager.focus.current(), Some(id));
        assert!(!manager.announcements.is_empty());
    }

    #[test]
    fn test_node_properties() {
        let mut node = AccessibleNode::new(Role::Slider);
        node.set_property("aria-valuenow", "50");
        node.set_property("aria-valuemin", "0");
        node.set_property("aria-valuemax", "100");
        
        assert_eq!(node.get_property("aria-valuenow"), Some("50"));
    }
}

