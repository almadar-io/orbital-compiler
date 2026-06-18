//! Pattern Definitions - Auto-generated from patterns-registry.json
//!
//! Source: packages/almadar-patterns/src/patterns-registry.json
//!
//! Last synced: 2026-06-18
//! To regenerate: npx tsx tools/almadar-pattern-sync/index.ts
//!
//! DO NOT EDIT MANUALLY - changes will be overwritten on next sync.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// All valid pattern types
pub static PATTERN_TYPES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        // Component Patterns
        "accordion",
        "alert",
        "animated-counter",
        "animated-graphic",
        "animated-reveal",
        "article-section",
        "aside",
        "avatar",
        "badge",
        "behavior-view",
        "button",
        "card",
        "carousel",
        "checkbox",
        "code-block",
        "community-links",
        "conditional-wrapper",
        "confetti-effect",
        "container",
        "content-section",
        "date-range-picker",
        "day-cell",
        "doc-toc",
        "edge-decoration",
        "file-tree",
        "flex",
        "floating-action-button",
        "form-actions",
        "geometric-pattern",
        "gradient-divider",
        "graph-view",
        "icon",
        "infinite-scroll-sentinel",
        "input",
        "install-box",
        "label",
        "law-reference-tooltip",
        "lightbox",
        "likert-scale",
        "map-view",
        "markdown-content",
        "marketing-footer",
        "matrix-question",
        "menu",
        "number-stepper",
        "option-constraint-group",
        "popover",
        "progress-bar",
        "progress-dots",
        "pull-quote",
        "pull-to-refresh",
        "qr-scanner",
        "quiz-block",
        "radio",
        "range-slider",
        "scaled-diagram",
        "select",
        "service-catalog",
        "side-panel",
        "simple-grid",
        "skeleton",
        "social-proof",
        "sparkline",
        "spinner",
        "stack",
        "star-rating",
        "step-flow",
        "svg-branch",
        "svg-connection",
        "svg-flow",
        "svg-lobe",
        "svg-mesh",
        "svg-morph",
        "svg-node",
        "svg-pulse",
        "svg-ring",
        "svg-shield",
        "svg-stack",
        "swipeable-row",
        "switch",
        "tag-cloud",
        "text-highlight",
        "textarea",
        "theme-toggle",
        "tooltip",
        "trait-frame",
        "trend-indicator",
        "typewriter-text",
        "typography",
        "upload-drop-zone",
        "vote-stack",
        // Container Patterns
        "confirm-dialog",
        "dialog",
        "drawer",
        "modal",
        "overlay",
        // Debug Patterns
        "runtime-debugger",
        // Display Patterns
        "book-chapter-view",
        "book-cover-page",
        "book-table-of-contents",
        "book-viewer",
        "branching-logic-builder",
        "calendar-grid",
        "case-study-card",
        "case-study-organism",
        "chart-legend",
        "content-renderer",
        "data-grid",
        "data-list",
        "detail-panel",
        "document-viewer",
        "entity-cards",
        "entity-list",
        "entity-table",
        "feature-card",
        "feature-grid",
        "feature-grid-organism",
        "flip-card",
        "jazari-state-machine",
        "line-chart",
        "master-detail-layout",
        "module-card",
        "orbital-visualization",
        "pricing-card",
        "pricing-grid",
        "pricing-organism",
        "reply-tree",
        "rich-block-editor",
        "section",
        "showcase-card",
        "showcase-organism",
        "sortable-list",
        "stat-card",
        "stat-display",
        "state-machine-view",
        "stats-grid",
        "stats-organism",
        "status-dot",
        "step-flow-organism",
        "svg-grid",
        "table-view",
        "team-card",
        "team-organism",
        "timeline",
        "version-diff",
        // Filter Patterns
        "doc-search",
        "filter-group",
        "filter-pill",
        "search-input",
        // Form Patterns
        "date-range-selector",
        "form",
        "form-field",
        "form-section",
        "form-section-header",
        "input-group",
        "relation-select",
        "repeatable-form-section",
        "signature-pad",
        "tag-input",
        // Game Patterns
        "action-button",
        "action-buttons",
        "action-palette",
        "action-tile",
        "battle-board",
        "battle-template",
        "builder-board",
        "canvas-effect",
        "castle-board",
        "castle-template",
        "choice-button",
        "classifier-board",
        "combat-log",
        "combo-counter",
        "control-button",
        "crafting-recipe",
        "d-pad",
        "damage-number",
        "dashboard-grid",
        "debugger-board",
        "dialogue-box",
        "dialogue-bubble",
        "enemy-plate",
        "event-handler-board",
        "event-log",
        "feature-renderer",
        "game-audio-provider",
        "game-audio-toggle",
        "game-canvas-2d",
        "game-canvas-3d",
        "game-canvas-3d-battle-template",
        "game-canvas-3d-castle-template",
        "game-canvas-3d-world-map-template",
        "game-hud",
        "game-menu",
        "game-over-screen",
        "game-shell",
        "game-template",
        "graph-canvas",
        "health-bar",
        "health-panel",
        "inventory-grid",
        "inventory-panel",
        "isometric-canvas",
        "item-slot",
        "mini-map",
        "negotiator-board",
        "object-rule-panel",
        "pattern-tile",
        "platformer-canvas",
        "positioned-canvas",
        "powerup-slots",
        "quest-tracker",
        "resource-bar",
        "resource-counter",
        "rule-editor",
        "score-board",
        "score-display",
        "sequence-bar",
        "sequencer-board",
        "simulation-canvas",
        "simulation-controls",
        "simulation-graph",
        "simulator-board",
        "sprite",
        "stat-badge",
        "state-architect-board",
        "state-indicator",
        "state-node",
        "status-effect",
        "timer-display",
        "trait-slot",
        "trait-state-viewer",
        "transition-arrow",
        "turn-indicator",
        "turn-panel",
        "uncontrolled-battle-board",
        "unit-command-bar",
        "variable-panel",
        "waypoint-marker",
        "world-map-board",
        "world-map-template",
        "xp-bar",
        // Header Patterns
        "header",
        "hero-organism",
        "hero-section",
        "page-header",
        "section-header",
        // Layout Patterns
        "box",
        "center",
        "divider",
        "drawer-slot",
        "flip-container",
        "grid",
        "master-detail",
        "modal-slot",
        "spacer",
        "split",
        "split-pane",
        "split-section",
        "time-slot-cell",
        "toast-slot",
        "ui-slot-renderer",
        // Media Patterns
        "media-gallery",
        // Navigation Patterns
        "book-nav-bar",
        "breadcrumb",
        "cta-banner",
        "doc-breadcrumb",
        "doc-pagination",
        "doc-sidebar",
        "navigation",
        "pagination",
        "sidebar",
        "tabbed-container",
        "tabs",
        "wizard-container",
        "wizard-navigation",
        "wizard-progress",
        // State Patterns
        "empty-state",
        "error-boundary",
        "error-state",
        "loading-state",
        "notification",
        "violation-alert",
        // Template Patterns
        "about-page-template",
        "auth-layout",
        "counter-template",
        "dashboard-layout",
        "feature-detail-page-template",
        "generic-app-template",
        "landing-page-template",
        "pricing-page-template",
        // Visualization Patterns
        "chart",
        "meter",
    ]
    .into_iter()
    .collect()
});

/// Pattern categories
pub static PATTERN_CATEGORIES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "component",
        "container",
        "debug",
        "display",
        "filter",
        "form",
        "game",
        "header",
        "layout",
        "media",
        "navigation",
        "state",
        "template",
        "visualization",
    ]
    .into_iter()
    .collect()
});

/// Pattern to category mapping
pub static PATTERN_TO_CATEGORY: LazyLock<HashMap<&'static str, &'static str>> =
    LazyLock::new(|| {
        [
            // Component
            ("accordion", "component"),
            ("alert", "component"),
            ("animated-counter", "component"),
            ("animated-graphic", "component"),
            ("animated-reveal", "component"),
            ("article-section", "component"),
            ("aside", "component"),
            ("avatar", "component"),
            ("badge", "component"),
            ("behavior-view", "component"),
            ("button", "component"),
            ("card", "component"),
            ("carousel", "component"),
            ("checkbox", "component"),
            ("code-block", "component"),
            ("community-links", "component"),
            ("conditional-wrapper", "component"),
            ("confetti-effect", "component"),
            ("container", "component"),
            ("content-section", "component"),
            ("date-range-picker", "component"),
            ("day-cell", "component"),
            ("doc-toc", "component"),
            ("edge-decoration", "component"),
            ("file-tree", "component"),
            ("flex", "component"),
            ("floating-action-button", "component"),
            ("form-actions", "component"),
            ("geometric-pattern", "component"),
            ("gradient-divider", "component"),
            ("graph-view", "component"),
            ("icon", "component"),
            ("infinite-scroll-sentinel", "component"),
            ("input", "component"),
            ("install-box", "component"),
            ("label", "component"),
            ("law-reference-tooltip", "component"),
            ("lightbox", "component"),
            ("likert-scale", "component"),
            ("map-view", "component"),
            ("markdown-content", "component"),
            ("marketing-footer", "component"),
            ("matrix-question", "component"),
            ("menu", "component"),
            ("number-stepper", "component"),
            ("option-constraint-group", "component"),
            ("popover", "component"),
            ("progress-bar", "component"),
            ("progress-dots", "component"),
            ("pull-quote", "component"),
            ("pull-to-refresh", "component"),
            ("qr-scanner", "component"),
            ("quiz-block", "component"),
            ("radio", "component"),
            ("range-slider", "component"),
            ("scaled-diagram", "component"),
            ("select", "component"),
            ("service-catalog", "component"),
            ("side-panel", "component"),
            ("simple-grid", "component"),
            ("skeleton", "component"),
            ("social-proof", "component"),
            ("sparkline", "component"),
            ("spinner", "component"),
            ("stack", "component"),
            ("star-rating", "component"),
            ("step-flow", "component"),
            ("svg-branch", "component"),
            ("svg-connection", "component"),
            ("svg-flow", "component"),
            ("svg-lobe", "component"),
            ("svg-mesh", "component"),
            ("svg-morph", "component"),
            ("svg-node", "component"),
            ("svg-pulse", "component"),
            ("svg-ring", "component"),
            ("svg-shield", "component"),
            ("svg-stack", "component"),
            ("swipeable-row", "component"),
            ("switch", "component"),
            ("tag-cloud", "component"),
            ("text-highlight", "component"),
            ("textarea", "component"),
            ("theme-toggle", "component"),
            ("tooltip", "component"),
            ("trait-frame", "component"),
            ("trend-indicator", "component"),
            ("typewriter-text", "component"),
            ("typography", "component"),
            ("upload-drop-zone", "component"),
            ("vote-stack", "component"),
            // Container
            ("confirm-dialog", "container"),
            ("dialog", "container"),
            ("drawer", "container"),
            ("modal", "container"),
            ("overlay", "container"),
            // Debug
            ("runtime-debugger", "debug"),
            // Display
            ("book-chapter-view", "display"),
            ("book-cover-page", "display"),
            ("book-table-of-contents", "display"),
            ("book-viewer", "display"),
            ("branching-logic-builder", "display"),
            ("calendar-grid", "display"),
            ("case-study-card", "display"),
            ("case-study-organism", "display"),
            ("chart-legend", "display"),
            ("content-renderer", "display"),
            ("data-grid", "display"),
            ("data-list", "display"),
            ("detail-panel", "display"),
            ("document-viewer", "display"),
            ("entity-cards", "display"),
            ("entity-list", "display"),
            ("entity-table", "display"),
            ("feature-card", "display"),
            ("feature-grid", "display"),
            ("feature-grid-organism", "display"),
            ("flip-card", "display"),
            ("jazari-state-machine", "display"),
            ("line-chart", "display"),
            ("master-detail-layout", "display"),
            ("module-card", "display"),
            ("orbital-visualization", "display"),
            ("pricing-card", "display"),
            ("pricing-grid", "display"),
            ("pricing-organism", "display"),
            ("reply-tree", "display"),
            ("rich-block-editor", "display"),
            ("section", "display"),
            ("showcase-card", "display"),
            ("showcase-organism", "display"),
            ("sortable-list", "display"),
            ("stat-card", "display"),
            ("stat-display", "display"),
            ("state-machine-view", "display"),
            ("stats-grid", "display"),
            ("stats-organism", "display"),
            ("status-dot", "display"),
            ("step-flow-organism", "display"),
            ("svg-grid", "display"),
            ("table-view", "display"),
            ("team-card", "display"),
            ("team-organism", "display"),
            ("timeline", "display"),
            ("version-diff", "display"),
            // Filter
            ("doc-search", "filter"),
            ("filter-group", "filter"),
            ("filter-pill", "filter"),
            ("search-input", "filter"),
            // Form
            ("date-range-selector", "form"),
            ("form", "form"),
            ("form-field", "form"),
            ("form-section", "form"),
            ("form-section-header", "form"),
            ("input-group", "form"),
            ("relation-select", "form"),
            ("repeatable-form-section", "form"),
            ("signature-pad", "form"),
            ("tag-input", "form"),
            // Game
            ("action-button", "game"),
            ("action-buttons", "game"),
            ("action-palette", "game"),
            ("action-tile", "game"),
            ("battle-board", "game"),
            ("battle-template", "game"),
            ("builder-board", "game"),
            ("canvas-effect", "game"),
            ("castle-board", "game"),
            ("castle-template", "game"),
            ("choice-button", "game"),
            ("classifier-board", "game"),
            ("combat-log", "game"),
            ("combo-counter", "game"),
            ("control-button", "game"),
            ("crafting-recipe", "game"),
            ("d-pad", "game"),
            ("damage-number", "game"),
            ("dashboard-grid", "game"),
            ("debugger-board", "game"),
            ("dialogue-box", "game"),
            ("dialogue-bubble", "game"),
            ("enemy-plate", "game"),
            ("event-handler-board", "game"),
            ("event-log", "game"),
            ("feature-renderer", "game"),
            ("game-audio-provider", "game"),
            ("game-audio-toggle", "game"),
            ("game-canvas-2d", "game"),
            ("game-canvas-3d", "game"),
            ("game-canvas-3d-battle-template", "game"),
            ("game-canvas-3d-castle-template", "game"),
            ("game-canvas-3d-world-map-template", "game"),
            ("game-hud", "game"),
            ("game-menu", "game"),
            ("game-over-screen", "game"),
            ("game-shell", "game"),
            ("game-template", "game"),
            ("graph-canvas", "game"),
            ("health-bar", "game"),
            ("health-panel", "game"),
            ("inventory-grid", "game"),
            ("inventory-panel", "game"),
            ("isometric-canvas", "game"),
            ("item-slot", "game"),
            ("mini-map", "game"),
            ("negotiator-board", "game"),
            ("object-rule-panel", "game"),
            ("pattern-tile", "game"),
            ("platformer-canvas", "game"),
            ("positioned-canvas", "game"),
            ("powerup-slots", "game"),
            ("quest-tracker", "game"),
            ("resource-bar", "game"),
            ("resource-counter", "game"),
            ("rule-editor", "game"),
            ("score-board", "game"),
            ("score-display", "game"),
            ("sequence-bar", "game"),
            ("sequencer-board", "game"),
            ("simulation-canvas", "game"),
            ("simulation-controls", "game"),
            ("simulation-graph", "game"),
            ("simulator-board", "game"),
            ("sprite", "game"),
            ("stat-badge", "game"),
            ("state-architect-board", "game"),
            ("state-indicator", "game"),
            ("state-node", "game"),
            ("status-effect", "game"),
            ("timer-display", "game"),
            ("trait-slot", "game"),
            ("trait-state-viewer", "game"),
            ("transition-arrow", "game"),
            ("turn-indicator", "game"),
            ("turn-panel", "game"),
            ("uncontrolled-battle-board", "game"),
            ("unit-command-bar", "game"),
            ("variable-panel", "game"),
            ("waypoint-marker", "game"),
            ("world-map-board", "game"),
            ("world-map-template", "game"),
            ("xp-bar", "game"),
            // Header
            ("header", "header"),
            ("hero-organism", "header"),
            ("hero-section", "header"),
            ("page-header", "header"),
            ("section-header", "header"),
            // Layout
            ("box", "layout"),
            ("center", "layout"),
            ("divider", "layout"),
            ("drawer-slot", "layout"),
            ("flip-container", "layout"),
            ("grid", "layout"),
            ("master-detail", "layout"),
            ("modal-slot", "layout"),
            ("spacer", "layout"),
            ("split", "layout"),
            ("split-pane", "layout"),
            ("split-section", "layout"),
            ("time-slot-cell", "layout"),
            ("toast-slot", "layout"),
            ("ui-slot-renderer", "layout"),
            // Media
            ("media-gallery", "media"),
            // Navigation
            ("book-nav-bar", "navigation"),
            ("breadcrumb", "navigation"),
            ("cta-banner", "navigation"),
            ("doc-breadcrumb", "navigation"),
            ("doc-pagination", "navigation"),
            ("doc-sidebar", "navigation"),
            ("navigation", "navigation"),
            ("pagination", "navigation"),
            ("sidebar", "navigation"),
            ("tabbed-container", "navigation"),
            ("tabs", "navigation"),
            ("wizard-container", "navigation"),
            ("wizard-navigation", "navigation"),
            ("wizard-progress", "navigation"),
            // State
            ("empty-state", "state"),
            ("error-boundary", "state"),
            ("error-state", "state"),
            ("loading-state", "state"),
            ("notification", "state"),
            ("violation-alert", "state"),
            // Template
            ("about-page-template", "template"),
            ("auth-layout", "template"),
            ("counter-template", "template"),
            ("dashboard-layout", "template"),
            ("feature-detail-page-template", "template"),
            ("generic-app-template", "template"),
            ("landing-page-template", "template"),
            ("pricing-page-template", "template"),
            // Visualization
            ("chart", "visualization"),
            ("meter", "visualization"),
        ]
        .into_iter()
        .collect()
    });

/// Valid itemAction props (to detect hallucinated props from LLMs)
pub static VALID_ITEM_ACTION_PROPS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "label", "event", "icon", "variant", "disabled", "hidden", "confirm", "payload", "href",
    ]
    .into_iter()
    .collect()
});

/// Required props for patterns that need them
pub static PATTERN_REQUIRED_PROPS: LazyLock<HashMap<&'static str, Vec<&'static str>>> =
    LazyLock::new(|| {
        [
            ("about-page-template", vec!["entity"]),
            ("accordion", vec!["items"]),
            ("action-button", vec!["label"]),
            ("action-buttons", vec!["buttons"]),
            ("action-palette", vec!["actions"]),
            ("action-tile", vec!["action"]),
            ("animated-counter", vec!["value"]),
            ("animated-reveal", vec!["children"]),
            ("article-section", vec!["title", "children"]),
            ("battle-template", vec!["entity"]),
            ("behavior-view", vec!["data"]),
            ("book-chapter-view", vec!["chapter"]),
            ("book-cover-page", vec!["title"]),
            ("book-nav-bar", vec!["currentPage", "totalPages"]),
            ("book-table-of-contents", vec!["parts"]),
            ("branching-logic-builder", vec!["questions", "rules"]),
            ("breadcrumb", vec!["items"]),
            ("canvas-effect", vec!["actionType", "x", "y"]),
            ("carousel", vec!["items"]),
            ("case-study-card", vec!["title", "description", "category", "href"]),
            ("castle-template", vec!["entity"]),
            ("center", vec!["children"]),
            ("chart-legend", vec!["items"]),
            ("choice-button", vec!["text"]),
            ("combat-log", vec!["events"]),
            ("combo-counter", vec!["combo"]),
            ("conditional-wrapper", vec!["context", "children"]),
            ("confetti-effect", vec!["trigger"]),
            ("confirm-dialog", vec!["title"]),
            ("content-section", vec!["children"]),
            ("counter-template", vec!["entity"]),
            ("crafting-recipe", vec!["inputs", "output"]),
            ("cta-banner", vec!["title"]),
            ("damage-number", vec!["value"]),
            ("dashboard-grid", vec!["cells"]),
            ("data-grid", vec!["entity"]),
            ("data-list", vec!["entity"]),
            ("detail-panel", vec!["fields"]),
            ("dialogue-box", vec!["dialogue"]),
            ("dialogue-bubble", vec!["text"]),
            ("doc-breadcrumb", vec!["items"]),
            ("doc-sidebar", vec!["items"]),
            ("doc-toc", vec!["items"]),
            ("enemy-plate", vec!["name", "health", "maxHealth"]),
            ("entity-cards", vec!["fields"]),
            ("entity-list", vec!["fields"]),
            ("entity-table", vec!["fields"]),
            ("error-boundary", vec!["children"]),
            ("event-log", vec!["entries"]),
            ("feature-card", vec!["title", "description"]),
            ("feature-detail-page-template", vec!["entity"]),
            ("feature-grid", vec!["items"]),
            ("feature-renderer", vec!["features"]),
            ("file-tree", vec!["tree"]),
            ("filter-group", vec!["entity", "filters"]),
            ("flex", vec!["children"]),
            ("flip-card", vec!["front", "back"]),
            ("flip-container", vec!["flipped", "children"]),
            ("form", vec!["fields"]),
            ("form-field", vec!["label", "children"]),
            ("form-section", vec!["fields"]),
            ("form-section-header", vec!["title"]),
            ("game-audio-provider", vec!["manifest", "children"]),
            ("game-canvas-3d-battle-template", vec!["entity"]),
            ("game-canvas-3d-castle-template", vec!["entity"]),
            ("game-canvas-3d-world-map-template", vec!["entity"]),
            ("game-menu", vec!["title"]),
            ("game-over-screen", vec!["title"]),
            ("game-template", vec!["entity", "children"]),
            ("generic-app-template", vec!["entity", "title", "children"]),
            ("graph-view", vec!["nodes", "edges"]),
            ("grid", vec!["children"]),
            ("health-bar", vec!["current", "max"]),
            ("health-panel", vec!["current", "max"]),
            ("hero-section", vec!["title", "subtitle"]),
            ("infinite-scroll-sentinel", vec!["loadMoreEvent"]),
            ("install-box", vec!["command"]),
            ("inventory-grid", vec!["items"]),
            ("inventory-panel", vec!["items", "slots", "columns"]),
            ("landing-page-template", vec!["entity"]),
            ("law-reference-tooltip", vec!["reference", "children"]),
            ("lightbox", vec!["images"]),
            ("line-chart", vec!["data"]),
            ("markdown-content", vec!["content"]),
            ("marketing-footer", vec!["columns"]),
            ("master-detail", vec!["masterFields"]),
            ("master-detail-layout", vec!["master", "detail"]),
            ("matrix-question", vec!["rows"]),
            ("menu", vec!["trigger", "items"]),
            ("meter", vec!["value"]),
            ("module-card", vec!["data"]),
            ("navigation", vec!["items"]),
            ("notification", vec!["message"]),
            ("object-rule-panel", vec!["object", "onRulesChange"]),
            ("option-constraint-group", vec!["groupId", "title", "options"]),
            ("pagination", vec!["currentPage", "totalPages"]),
            ("popover", vec!["content", "children"]),
            ("positioned-canvas", vec!["items"]),
            ("powerup-slots", vec!["active"]),
            ("pricing-card", vec!["name", "price", "features", "action"]),
            ("pricing-grid", vec!["plans"]),
            ("pricing-page-template", vec!["entity"]),
            ("progress-bar", vec!["value"]),
            ("progress-dots", vec!["count", "currentIndex"]),
            ("pull-quote", vec!["children"]),
            ("pull-to-refresh", vec!["refreshEvent", "children"]),
            ("quest-tracker", vec!["quests"]),
            ("quiz-block", vec!["question", "answer"]),
            ("relation-select", vec!["options"]),
            ("repeatable-form-section", vec!["sectionType", "title", "items", "renderItem"]),
            ("reply-tree", vec!["nodes"]),
            ("resource-bar", vec!["resources"]),
            ("resource-counter", vec!["label", "value"]),
            ("rule-editor", vec!["rule", "availableEvents", "availableActions", "onChange"]),
            ("scaled-diagram", vec!["children"]),
            ("score-board", vec!["score"]),
            ("score-display", vec!["value"]),
            ("section", vec!["children"]),
            ("section-header", vec!["title"]),
            ("sequence-bar", vec!["slots", "maxSlots", "onSlotDrop", "onSlotRemove"]),
            ("service-catalog", vec!["services"]),
            ("showcase-card", vec!["title", "image"]),
            ("side-panel", vec!["title", "children", "isOpen", "onClose"]),
            ("sidebar", vec!["items"]),
            ("simple-grid", vec!["children"]),
            ("simulation-canvas", vec!["preset", "running"]),
            ("simulation-controls", vec!["running", "speed", "parameters", "onPlay", "onPause", "onStep", "onReset", "onSpeedChange", "onParameterChange"]),
            ("simulation-graph", vec!["label", "unit", "data"]),
            ("social-proof", vec!["items"]),
            ("sortable-list", vec!["items", "renderItem", "reorderEvent"]),
            ("sparkline", vec!["data"]),
            ("split", vec!["children"]),
            ("split-pane", vec!["left", "right"]),
            ("split-section", vec!["title", "description"]),
            ("sprite", vec!["spritesheet", "frameWidth", "frameHeight", "frame", "x", "y"]),
            ("stat-badge", vec!["label"]),
            ("stat-card", vec!["value", "label"]),
            ("stat-display", vec!["label", "value"]),
            ("state-indicator", vec!["state"]),
            ("state-node", vec!["name", "position"]),
            ("stats-grid", vec!["stats"]),
            ("status-effect", vec!["icon"]),
            ("step-flow", vec!["steps"]),
            ("swipeable-row", vec!["children"]),
            ("tabbed-container", vec!["tabs"]),
            ("table-view", vec!["entity"]),
            ("tag-cloud", vec!["tags"]),
            ("tag-input", vec!["value"]),
            ("team-card", vec!["name", "role", "bio"]),
            ("text-highlight", vec!["highlightType", "children"]),
            ("time-slot-cell", vec!["time"]),
            ("timeline", vec!["fields"]),
            ("timer-display", vec!["seconds"]),
            ("tooltip", vec!["content", "children"]),
            ("trait-frame", vec!["traitName"]),
            ("trait-slot", vec!["slotNumber"]),
            ("trait-state-viewer", vec!["trait"]),
            ("transition-arrow", vec!["from", "to", "eventLabel"]),
            ("turn-indicator", vec!["currentTurn"]),
            ("turn-panel", vec!["currentTurn"]),
            ("typewriter-text", vec!["text"]),
            ("unit-command-bar", vec!["commands"]),
            ("variable-panel", vec!["entityName", "variables"]),
            ("version-diff", vec!["revisions"]),
            ("violation-alert", vec!["violation"]),
            ("vote-stack", vec!["count"]),
            ("wizard-container", vec!["steps"]),
            ("wizard-navigation", vec!["currentStep", "totalSteps"]),
            ("wizard-progress", vec!["currentStep"]),
            ("world-map-template", vec!["entity"]),
            ("xp-bar", vec!["current", "max"]),
        ]
        .into_iter()
        .collect()
    });

/// Valid props per pattern (from registry propsSchema)
pub static PATTERN_PROPS_SCHEMA: LazyLock<HashMap<&'static str, HashSet<&'static str>>> =
    LazyLock::new(|| {
        [
            ("about-page-template", ["className", "entity"].into_iter().collect()),
            (
                "accordion",
                [
                    "className",
                    "defaultOpen",
                    "defaultOpenItems",
                    "items",
                    "multiple",
                    "onItemToggle",
                    "openItems",
                    "toggleEvent",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "action-button",
                [
                    "className",
                    "cooldown",
                    "disabled",
                    "hotkey",
                    "icon",
                    "label",
                    "onClick",
                    "size",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            ("action-buttons", ["actionEvent", "buttons", "className", "disabled", "layout", "onAction", "size"].into_iter().collect()),
            (
                "action-palette",
                [
                    "actions",
                    "allowDuplicates",
                    "categoryColors",
                    "className",
                    "label",
                    "size",
                    "usedActionIds",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "action-tile",
                [
                    "action",
                    "activeFilters",
                    "categoryColors",
                    "className",
                    "disabled",
                    "error",
                    "isLoading",
                    "page",
                    "pageSize",
                    "searchValue",
                    "selectedIds",
                    "size",
                    "sortBy",
                    "sortDirection",
                    "totalCount",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "alert",
                [
                    "actions",
                    "children",
                    "className",
                    "dismissEvent",
                    "dismissible",
                    "message",
                    "onClose",
                    "onDismiss",
                    "title",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            ("animated-counter", ["className", "duration", "prefix", "suffix", "value"].into_iter().collect()),
            (
                "animated-graphic",
                [
                    "alt",
                    "animate",
                    "animation",
                    "className",
                    "delay",
                    "duration",
                    "easing",
                    "fillColor",
                    "height",
                    "src",
                    "strokeColor",
                    "svgContent",
                    "width",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "animated-reveal",
                [
                    "animate",
                    "animation",
                    "children",
                    "className",
                    "delay",
                    "duration",
                    "easing",
                    "once",
                    "threshold",
                    "trigger",
                ]
                .into_iter()
                .collect(),
            ),
            ("article-section", ["children", "className", "maxWidth", "title"].into_iter().collect()),
            ("aside", ["children", "className"].into_iter().collect()),
            ("auth-layout", ["appName", "backgroundImage", "brandingContent", "logo", "showBranding"].into_iter().collect()),
            (
                "avatar",
                [
                    "action",
                    "actionPayload",
                    "alt",
                    "badge",
                    "className",
                    "icon",
                    "initials",
                    "name",
                    "onClick",
                    "size",
                    "src",
                    "status",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "badge",
                [
                    "amount",
                    "className",
                    "icon",
                    "label",
                    "onRemove",
                    "removeLabel",
                    "size",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "battle-board",
                [
                    "actions",
                    "activeFilters",
                    "assetManifest",
                    "attackEvent",
                    "calculateDamage",
                    "cancelEvent",
                    "className",
                    "effectSpriteUrls",
                    "endTurnEvent",
                    "entity",
                    "error",
                    "features",
                    "gameEndEvent",
                    "gameOverOverlay",
                    "hasActiveEffects",
                    "header",
                    "isLoading",
                    "onAttack",
                    "onDrawEffects",
                    "onGameEnd",
                    "onUnitMove",
                    "overlay",
                    "page",
                    "pageSize",
                    "playAgainEvent",
                    "resolveUnitFrame",
                    "scale",
                    "searchValue",
                    "selectedIds",
                    "sidebar",
                    "sortBy",
                    "sortDirection",
                    "tileClickEvent",
                    "tiles",
                    "totalCount",
                    "unitClickEvent",
                    "unitScale",
                    "units",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "battle-template",
                [
                    "assetManifest",
                    "className",
                    "entity",
                    "features",
                    "scale",
                    "tiles",
                    "unitScale",
                    "units",
                ]
                .into_iter()
                .collect(),
            ),
            ("behavior-view", ["data"].into_iter().collect()),
            ("book-chapter-view", ["chapter", "className", "direction", "orbitalSchema"].into_iter().collect()),
            ("book-cover-page", ["author", "className", "coverImageUrl", "direction", "subtitle", "title"].into_iter().collect()),
            ("book-nav-bar", ["chapterTitle", "className", "currentPage", "direction", "totalPages"].into_iter().collect()),
            ("book-table-of-contents", ["className", "currentChapterId", "direction", "parts"].into_iter().collect()),
            (
                "book-viewer",
                [
                    "activeFilters",
                    "className",
                    "entity",
                    "error",
                    "fieldMap",
                    "initialPage",
                    "isLoading",
                    "page",
                    "pageSize",
                    "searchValue",
                    "selectedIds",
                    "sortBy",
                    "sortDirection",
                    "totalCount",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "box",
                [
                    "action",
                    "actionPayload",
                    "as",
                    "bg",
                    "border",
                    "children",
                    "className",
                    "display",
                    "fullHeight",
                    "fullWidth",
                    "hoverEvent",
                    "margin",
                    "marginX",
                    "marginY",
                    "maxWidth",
                    "overflow",
                    "padding",
                    "paddingX",
                    "paddingY",
                    "position",
                    "rounded",
                    "shadow",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "branching-logic-builder",
                [
                    "className",
                    "onRulesChange",
                    "questions",
                    "readOnly",
                    "rules",
                    "rulesChangeEvent",
                ]
                .into_iter()
                .collect(),
            ),
            ("breadcrumb", ["className", "items", "maxItems", "separator"].into_iter().collect()),
            (
                "builder-board",
                [
                    "activeFilters",
                    "className",
                    "completeEvent",
                    "entity",
                    "error",
                    "isLoading",
                    "page",
                    "pageSize",
                    "searchValue",
                    "selectedIds",
                    "sortBy",
                    "sortDirection",
                    "totalCount",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "button",
                [
                    "action",
                    "actionPayload",
                    "className",
                    "disabled",
                    "icon",
                    "iconRight",
                    "isLoading",
                    "label",
                    "leftIcon",
                    "rightIcon",
                    "size",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "calendar-grid",
                [
                    "className",
                    "dayWindow",
                    "events",
                    "longPressEvent",
                    "longPressPayload",
                    "onDayClick",
                    "onEventClick",
                    "onSlotClick",
                    "swipeLeftEvent",
                    "swipeRightEvent",
                    "timeSlots",
                    "weekStart",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "canvas-effect",
                [
                    "actionType",
                    "assetBaseUrl",
                    "assetManifest",
                    "className",
                    "completeEvent",
                    "duration",
                    "effectSpriteUrl",
                    "error",
                    "height",
                    "intensity",
                    "isLoading",
                    "onComplete",
                    "width",
                    "x",
                    "y",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "card",
                [
                    "action",
                    "children",
                    "className",
                    "loading",
                    "look",
                    "padding",
                    "shadow",
                    "subtitle",
                    "title",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "carousel",
                [
                    "autoPlay",
                    "autoPlayInterval",
                    "children",
                    "className",
                    "items",
                    "loop",
                    "renderItem",
                    "showArrows",
                    "showDots",
                    "slideChangeEvent",
                    "slideChangePayload",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "case-study-card",
                [
                    "category",
                    "categoryColor",
                    "className",
                    "description",
                    "href",
                    "linkLabel",
                    "title",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "case-study-organism",
                [
                    "activeFilters",
                    "className",
                    "entity",
                    "error",
                    "heading",
                    "isLoading",
                    "page",
                    "pageSize",
                    "searchValue",
                    "selectedIds",
                    "sortBy",
                    "sortDirection",
                    "subtitle",
                    "totalCount",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "castle-board",
                [
                    "assetManifest",
                    "className",
                    "entity",
                    "featureClickEvent",
                    "features",
                    "footer",
                    "header",
                    "onFeatureClick",
                    "onTileClick",
                    "onUnitClick",
                    "overlay",
                    "scale",
                    "sidePanel",
                    "tileClickEvent",
                    "tiles",
                    "unitClickEvent",
                    "units",
                ]
                .into_iter()
                .collect(),
            ),
            ("castle-template", ["assetManifest", "className", "entity", "features", "scale", "tiles", "units"].into_iter().collect()),
            (
                "center",
                [
                    "as",
                    "children",
                    "className",
                    "fullHeight",
                    "fullWidth",
                    "horizontal",
                    "inline",
                    "minHeight",
                    "style",
                    "vertical",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "chart",
                [
                    "actions",
                    "chartType",
                    "className",
                    "data",
                    "drillEvent",
                    "entity",
                    "error",
                    "height",
                    "isLoading",
                    "look",
                    "scatterData",
                    "series",
                    "showLegend",
                    "showValues",
                    "stack",
                    "subtitle",
                    "timeAxis",
                    "title",
                ]
                .into_iter()
                .collect(),
            ),
            ("chart-legend", ["className", "direction", "items"].into_iter().collect()),
            ("checkbox", ["checked", "className", "defaultChecked", "label", "onChange"].into_iter().collect()),
            ("choice-button", ["className", "disabled", "index", "onClick", "selected", "text"].into_iter().collect()),
            (
                "classifier-board",
                [
                    "activeFilters",
                    "className",
                    "completeEvent",
                    "entity",
                    "error",
                    "isLoading",
                    "page",
                    "pageSize",
                    "searchValue",
                    "selectedIds",
                    "sortBy",
                    "sortDirection",
                    "totalCount",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "code-block",
                [
                    "actions",
                    "className",
                    "code",
                    "diff",
                    "editable",
                    "error",
                    "errorLines",
                    "files",
                    "foldable",
                    "isLoading",
                    "language",
                    "maxHeight",
                    "mode",
                    "newValue",
                    "oldValue",
                    "onChange",
                    "showCopy",
                    "showCopyButton",
                    "showLanguageBadge",
                    "showLineNumbers",
                    "title",
                    "wordWrap",
                ]
                .into_iter()
                .collect(),
            ),
            ("combat-log", ["autoScroll", "className", "events", "maxVisible", "showTimestamps", "title"].into_iter().collect()),
            ("combo-counter", ["className", "combo", "multiplier", "size", "streak"].into_iter().collect()),
            ("community-links", ["className", "discord", "github", "heading", "subtitle", "twitter"].into_iter().collect()),
            ("conditional-wrapper", ["animate", "children", "condition", "context", "fallback"].into_iter().collect()),
            ("confetti-effect", ["className", "duration", "particleCount", "trigger"].into_iter().collect()),
            (
                "confirm-dialog",
                [
                    "cancelLabel",
                    "cancelText",
                    "className",
                    "confirmLabel",
                    "confirmText",
                    "description",
                    "entity",
                    "error",
                    "isLoading",
                    "isOpen",
                    "message",
                    "onClose",
                    "onConfirm",
                    "size",
                    "title",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            ("container", ["as", "center", "children", "className", "maxWidth", "padding", "size"].into_iter().collect()),
            (
                "content-renderer",
                [
                    "activeFilters",
                    "className",
                    "content",
                    "direction",
                    "error",
                    "isLoading",
                    "page",
                    "pageSize",
                    "searchValue",
                    "segments",
                    "selectedIds",
                    "sortBy",
                    "sortDirection",
                    "totalCount",
                ]
                .into_iter()
                .collect(),
            ),
            ("content-section", ["background", "children", "className", "id", "padding"].into_iter().collect()),
            (
                "control-button",
                [
                    "className",
                    "disabled",
                    "icon",
                    "label",
                    "onPress",
                    "onRelease",
                    "pressEvent",
                    "pressed",
                    "releaseEvent",
                    "shape",
                    "size",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "counter-template",
                [
                    "className",
                    "entity",
                    "onDecrement",
                    "onIncrement",
                    "onReset",
                    "showReset",
                    "size",
                    "title",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            ("crafting-recipe", ["canCraft", "className", "craftEvent", "inputs", "onCraft", "output"].into_iter().collect()),
            (
                "cta-banner",
                [
                    "align",
                    "background",
                    "className",
                    "primaryAction",
                    "secondaryAction",
                    "subtitle",
                    "title",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "d-pad",
                [
                    "className",
                    "directionEvent",
                    "disabled",
                    "includeDiagonals",
                    "onDirection",
                    "size",
                ]
                .into_iter()
                .collect(),
            ),
            ("damage-number", ["className", "size", "type", "value"].into_iter().collect()),
            (
                "dashboard-grid",
                [
                    "activeFilters",
                    "cells",
                    "className",
                    "columns",
                    "error",
                    "gap",
                    "isLoading",
                    "page",
                    "pageSize",
                    "searchValue",
                    "selectedIds",
                    "sortBy",
                    "sortDirection",
                    "totalCount",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "dashboard-layout",
                [
                    "appName",
                    "children",
                    "currentPath",
                    "headerActions",
                    "layoutMode",
                    "logo",
                    "navItems",
                    "notificationClickEvent",
                    "notifications",
                    "onNotificationClick",
                    "onSearchSubmit",
                    "onSignOut",
                    "searchEvent",
                    "showSearch",
                    "showThemeToggle",
                    "sidebarFooter",
                    "topBarActions",
                    "user",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "data-grid",
                [
                    "accepts",
                    "children",
                    "className",
                    "cols",
                    "columns",
                    "dndItemIdField",
                    "dndRoot",
                    "dragGroup",
                    "dropEvent",
                    "entity",
                    "error",
                    "fields",
                    "gap",
                    "hasMore",
                    "imageField",
                    "infiniteScroll",
                    "isLoading",
                    "itemActions",
                    "loadMoreEvent",
                    "look",
                    "maxInlineActions",
                    "minCardWidth",
                    "pageSize",
                    "positionEvent",
                    "renderItem",
                    "reorderEvent",
                    "scrollX",
                    "selectable",
                    "selectionEvent",
                    "sortable",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "data-list",
                [
                    "accepts",
                    "children",
                    "className",
                    "columns",
                    "currentUser",
                    "dndItemIdField",
                    "dndRoot",
                    "dragGroup",
                    "dropEvent",
                    "entity",
                    "error",
                    "fields",
                    "gap",
                    "groupBy",
                    "hasMore",
                    "infiniteScroll",
                    "isLoading",
                    "itemActions",
                    "itemClickEvent",
                    "loadMoreEvent",
                    "longPressEvent",
                    "look",
                    "maxInlineActions",
                    "pageSize",
                    "positionEvent",
                    "renderItem",
                    "reorderEvent",
                    "reorderable",
                    "senderField",
                    "sortable",
                    "swipeLeftActions",
                    "swipeLeftEvent",
                    "swipeRightActions",
                    "swipeRightEvent",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "date-range-picker",
                [
                    "className",
                    "event",
                    "from",
                    "fromLabel",
                    "onChange",
                    "presets",
                    "to",
                    "toLabel",
                ]
                .into_iter()
                .collect(),
            ),
            ("date-range-selector", ["className", "onSelect", "options", "selected"].into_iter().collect()),
            ("day-cell", ["className", "date", "isToday", "onClick"].into_iter().collect()),
            (
                "debugger-board",
                [
                    "activeFilters",
                    "className",
                    "completeEvent",
                    "entity",
                    "error",
                    "isLoading",
                    "page",
                    "pageSize",
                    "searchValue",
                    "selectedIds",
                    "sortBy",
                    "sortDirection",
                    "totalCount",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "detail-panel",
                [
                    "actions",
                    "activeFilters",
                    "avatar",
                    "className",
                    "displayFields",
                    "entity",
                    "error",
                    "fieldNames",
                    "fields",
                    "footer",
                    "initialData",
                    "isLoading",
                    "mode",
                    "page",
                    "pageSize",
                    "position",
                    "searchValue",
                    "sections",
                    "selectedIds",
                    "showActions",
                    "slideOver",
                    "sortBy",
                    "sortDirection",
                    "status",
                    "subtitle",
                    "title",
                    "totalCount",
                    "width",
                ]
                .into_iter()
                .collect(),
            ),
            ("dialog", ["children", "className"].into_iter().collect()),
            (
                "dialogue-box",
                [
                    "advanceEvent",
                    "choiceEvent",
                    "className",
                    "completeEvent",
                    "dialogue",
                    "onAdvance",
                    "onChoice",
                    "onComplete",
                    "position",
                    "typewriterSpeed",
                ]
                .into_iter()
                .collect(),
            ),
            ("dialogue-bubble", ["className", "portrait", "position", "speaker", "text"].into_iter().collect()),
            ("divider", ["className", "label", "orientation", "variant"].into_iter().collect()),
            ("doc-breadcrumb", ["className", "items"].into_iter().collect()),
            ("doc-pagination", ["className", "next", "prev"].into_iter().collect()),
            ("doc-search", ["className", "onSearch", "placeholder"].into_iter().collect()),
            ("doc-sidebar", ["className", "items"].into_iter().collect()),
            ("doc-toc", ["activeId", "className", "items"].into_iter().collect()),
            (
                "document-viewer",
                [
                    "actions",
                    "className",
                    "content",
                    "currentPage",
                    "documentType",
                    "documents",
                    "error",
                    "height",
                    "isLoading",
                    "showDownload",
                    "showPrint",
                    "showToolbar",
                    "src",
                    "title",
                    "totalPages",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "drawer",
                [
                    "children",
                    "className",
                    "closeEvent",
                    "closeOnEscape",
                    "closeOnOverlayClick",
                    "footer",
                    "isOpen",
                    "onClose",
                    "position",
                    "showCloseButton",
                    "title",
                    "width",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "drawer-slot",
                [
                    "children",
                    "className",
                    "entity",
                    "error",
                    "isLoading",
                    "position",
                    "size",
                    "title",
                ]
                .into_iter()
                .collect(),
            ),
            ("edge-decoration", ["className", "color", "opacity", "side", "strokeWidth", "variant", "width"].into_iter().collect()),
            (
                "empty-state",
                [
                    "actionEvent",
                    "actionLabel",
                    "className",
                    "description",
                    "destructive",
                    "icon",
                    "look",
                    "message",
                    "onAction",
                    "title",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            ("enemy-plate", ["className", "effects", "health", "level", "maxHealth", "name"].into_iter().collect()),
            (
                "entity-cards",
                [
                    "activeFilters",
                    "alignItems",
                    "children",
                    "className",
                    "columns",
                    "entity",
                    "error",
                    "fieldNames",
                    "fields",
                    "gap",
                    "imageField",
                    "isLoading",
                    "itemActions",
                    "maxCols",
                    "minCardWidth",
                    "page",
                    "pageSize",
                    "searchValue",
                    "selectedIds",
                    "showAvatar",
                    "showTotal",
                    "sortBy",
                    "sortDirection",
                    "totalCount",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "entity-list",
                [
                    "activeFilters",
                    "children",
                    "className",
                    "emptyMessage",
                    "entity",
                    "entityType",
                    "error",
                    "fieldNames",
                    "fields",
                    "isLoading",
                    "itemActions",
                    "page",
                    "pageSize",
                    "renderItem",
                    "searchValue",
                    "selectable",
                    "selectedIds",
                    "showDividers",
                    "sortBy",
                    "sortDirection",
                    "totalCount",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "entity-table",
                [
                    "activeFilters",
                    "bulkActions",
                    "className",
                    "columns",
                    "emptyAction",
                    "emptyDescription",
                    "emptyIcon",
                    "emptyTitle",
                    "entity",
                    "error",
                    "fields",
                    "headerActions",
                    "isLoading",
                    "itemActions",
                    "look",
                    "page",
                    "pageSize",
                    "rowActions",
                    "searchPlaceholder",
                    "searchValue",
                    "searchable",
                    "selectable",
                    "selectedIds",
                    "showTotal",
                    "sortBy",
                    "sortDirection",
                    "totalCount",
                ]
                .into_iter()
                .collect(),
            ),
            ("error-boundary", ["children", "className", "fallback", "onError"].into_iter().collect()),
            ("error-state", ["className", "description", "message", "onRetry", "retryEvent", "title"].into_iter().collect()),
            (
                "event-handler-board",
                [
                    "activeFilters",
                    "className",
                    "completeEvent",
                    "entity",
                    "error",
                    "isLoading",
                    "page",
                    "pageSize",
                    "playEvent",
                    "searchValue",
                    "selectedIds",
                    "sortBy",
                    "sortDirection",
                    "stepDurationMs",
                    "totalCount",
                ]
                .into_iter()
                .collect(),
            ),
            ("event-log", ["className", "entries", "label", "maxHeight"].into_iter().collect()),
            (
                "feature-card",
                [
                    "className",
                    "description",
                    "href",
                    "icon",
                    "linkLabel",
                    "size",
                    "title",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            ("feature-detail-page-template", ["className", "entity"].into_iter().collect()),
            ("feature-grid", ["className", "columns", "gap", "items"].into_iter().collect()),
            (
                "feature-grid-organism",
                [
                    "activeFilters",
                    "className",
                    "columns",
                    "entity",
                    "error",
                    "heading",
                    "isLoading",
                    "page",
                    "pageSize",
                    "searchValue",
                    "selectedIds",
                    "sortBy",
                    "sortDirection",
                    "subtitle",
                    "totalCount",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "feature-renderer",
                [
                    "cellSize",
                    "featureColors",
                    "features",
                    "offsetX",
                    "offsetZ",
                    "onFeatureClick",
                    "onFeatureHover",
                    "selectedFeatureIds",
                ]
                .into_iter()
                .collect(),
            ),
            ("file-tree", ["className", "indent", "onFileSelect", "selectedPath", "tree"].into_iter().collect()),
            (
                "filter-group",
                [
                    "className",
                    "entity",
                    "filters",
                    "isLoading",
                    "look",
                    "onClearAll",
                    "onFilterChange",
                    "query",
                    "showIcon",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "filter-pill",
                [
                    "className",
                    "clickEvent",
                    "icon",
                    "label",
                    "onClick",
                    "onRemove",
                    "removable",
                    "removeEvent",
                    "size",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "flex",
                [
                    "align",
                    "as",
                    "basis",
                    "children",
                    "className",
                    "direction",
                    "gap",
                    "grow",
                    "inline",
                    "justify",
                    "shrink",
                    "wrap",
                ]
                .into_iter()
                .collect(),
            ),
            ("flip-card", ["back", "className", "flipped", "front", "height", "onFlip"].into_iter().collect()),
            ("flip-container", ["children", "className", "flipped", "onClick"].into_iter().collect()),
            (
                "floating-action-button",
                [
                    "action",
                    "actionPayload",
                    "actions",
                    "className",
                    "icon",
                    "label",
                    "onClick",
                    "position",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "form",
                [
                    "cancelEvent",
                    "cancelLabel",
                    "children",
                    "className",
                    "conditionalFields",
                    "configPath",
                    "entity",
                    "error",
                    "evaluationContext",
                    "fields",
                    "gap",
                    "hiddenCalculations",
                    "initialData",
                    "isLoading",
                    "layout",
                    "mode",
                    "onCancel",
                    "onFieldChange",
                    "onSubmit",
                    "relationsData",
                    "relationsLoading",
                    "repeatable",
                    "sections",
                    "showCancel",
                    "showSubmit",
                    "submitEvent",
                    "submitLabel",
                    "title",
                    "violationTriggers",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "form-actions",
                [
                    "children",
                    "className",
                    "entity",
                    "filters",
                    "look",
                    "orientation",
                    "primary",
                    "secondary",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            ("form-field", ["children", "className", "error", "hint", "label", "required"].into_iter().collect()),
            (
                "form-section",
                [
                    "cancelEvent",
                    "cancelLabel",
                    "children",
                    "className",
                    "conditionalFields",
                    "configPath",
                    "entity",
                    "error",
                    "evaluationContext",
                    "fields",
                    "gap",
                    "hiddenCalculations",
                    "initialData",
                    "isLoading",
                    "layout",
                    "mode",
                    "onCancel",
                    "onFieldChange",
                    "onSubmit",
                    "relationsData",
                    "relationsLoading",
                    "repeatable",
                    "sections",
                    "showCancel",
                    "showSubmit",
                    "submitEvent",
                    "submitLabel",
                    "title",
                    "violationTriggers",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "form-section-header",
                [
                    "badge",
                    "badgeVariant",
                    "className",
                    "hasErrors",
                    "icon",
                    "isCollapsed",
                    "isComplete",
                    "onToggle",
                    "subtitle",
                    "title",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "game-audio-provider",
                [
                    "baseUrl",
                    "children",
                    "className",
                    "entity",
                    "error",
                    "initialMuted",
                    "isLoading",
                    "manifest",
                ]
                .into_iter()
                .collect(),
            ),
            ("game-audio-toggle", ["className", "entity", "error", "isLoading", "size"].into_iter().collect()),
            (
                "game-canvas-2d",
                [
                    "assetBaseUrl",
                    "backgroundImage",
                    "className",
                    "drawEvent",
                    "fps",
                    "height",
                    "onDraw",
                    "onTick",
                    "tickEvent",
                    "width",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "game-canvas-3d",
                [
                    "assetLoader",
                    "attackTargets",
                    "backgroundColor",
                    "cameraChangeEvent",
                    "cameraMode",
                    "canvasClickEvent",
                    "children",
                    "className",
                    "entity",
                    "error",
                    "events",
                    "featureClickEvent",
                    "featureRenderer",
                    "features",
                    "fogOfWar",
                    "isLoading",
                    "loadingMessage",
                    "onCanvasClick",
                    "onFeatureClick",
                    "onTileClick",
                    "onTileHover",
                    "onUnitAnimation",
                    "onUnitClick",
                    "orientation",
                    "overlay",
                    "preloadAssets",
                    "selectedTileIds",
                    "selectedUnitId",
                    "shadows",
                    "showCoordinates",
                    "showGrid",
                    "showTileInfo",
                    "tileClickEvent",
                    "tileHoverEvent",
                    "tileLeaveEvent",
                    "tileRenderer",
                    "tiles",
                    "unitAnimationEvent",
                    "unitClickEvent",
                    "unitRenderer",
                    "units",
                    "useInstancing",
                    "validMoves",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "game-canvas-3d-battle-template",
                [
                    "attackTargets",
                    "backgroundColor",
                    "cameraMode",
                    "className",
                    "endTurnEvent",
                    "entity",
                    "exitEvent",
                    "selectedUnitId",
                    "shadows",
                    "showGrid",
                    "showTurnIndicator",
                    "tileClickEvent",
                    "unitAttackEvent",
                    "unitClickEvent",
                    "unitMoveEvent",
                    "validMoves",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "game-canvas-3d-castle-template",
                [
                    "availableBuildSites",
                    "backgroundColor",
                    "buildEvent",
                    "buildingClickEvent",
                    "cameraMode",
                    "className",
                    "entity",
                    "exitEvent",
                    "recruitEvent",
                    "selectedBuildingId",
                    "selectedTileIds",
                    "shadows",
                    "showGrid",
                    "showHeader",
                    "unitClickEvent",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "game-canvas-3d-world-map-template",
                [
                    "attackTargets",
                    "backgroundColor",
                    "cameraChangeEvent",
                    "cameraMode",
                    "className",
                    "entity",
                    "exitEvent",
                    "featureClickEvent",
                    "selectedUnitId",
                    "shadows",
                    "showCoordinates",
                    "showGrid",
                    "showTileInfo",
                    "tileClickEvent",
                    "tileHoverEvent",
                    "tileLeaveEvent",
                    "unitClickEvent",
                    "validMoves",
                ]
                .into_iter()
                .collect(),
            ),
            ("game-hud", ["className", "elements", "items", "position", "size", "stats", "transparent"].into_iter().collect()),
            (
                "game-menu",
                [
                    "background",
                    "className",
                    "eventBus",
                    "logo",
                    "menuItems",
                    "onSelect",
                    "options",
                    "subtitle",
                    "title",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "game-over-screen",
                [
                    "actions",
                    "className",
                    "currentScore",
                    "eventBus",
                    "highScore",
                    "menuItems",
                    "message",
                    "onAction",
                    "stats",
                    "title",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            ("game-shell", ["appName", "children", "className", "hud", "showTopBar"].into_iter().collect()),
            (
                "game-template",
                [
                    "children",
                    "className",
                    "controls",
                    "debugPanel",
                    "entity",
                    "hud",
                    "showDebugPanel",
                    "title",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "generic-app-template",
                [
                    "children",
                    "className",
                    "entity",
                    "footer",
                    "headerActions",
                    "subtitle",
                    "title",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "geometric-pattern",
                [
                    "children",
                    "className",
                    "color",
                    "mode",
                    "opacity",
                    "scale",
                    "strokeWidth",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            ("gradient-divider", ["className", "color"].into_iter().collect()),
            (
                "graph-canvas",
                [
                    "actions",
                    "className",
                    "draggable",
                    "edges",
                    "entity",
                    "error",
                    "height",
                    "interactive",
                    "isLoading",
                    "layout",
                    "nodeClickEvent",
                    "nodes",
                    "onNodeClick",
                    "showLabels",
                    "title",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "graph-view",
                [
                    "className",
                    "edges",
                    "height",
                    "nodes",
                    "onNodeClick",
                    "onNodeHover",
                    "showLabels",
                    "width",
                    "zoomToFit",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "grid",
                [
                    "alignItems",
                    "as",
                    "children",
                    "className",
                    "colGap",
                    "cols",
                    "flow",
                    "gap",
                    "justifyItems",
                    "rowGap",
                    "rows",
                    "style",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "header",
                [
                    "actions",
                    "brandName",
                    "className",
                    "error",
                    "isLoading",
                    "isMenuOpen",
                    "logo",
                    "logoSrc",
                    "look",
                    "navigationItems",
                    "onLogoClick",
                    "onMenuToggle",
                    "onSearch",
                    "onUserClick",
                    "searchPlaceholder",
                    "showMenuToggle",
                    "showSearch",
                    "sticky",
                    "userAvatar",
                    "userName",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            ("health-bar", ["animated", "className", "current", "format", "max", "size"].into_iter().collect()),
            (
                "health-panel",
                [
                    "className",
                    "current",
                    "effects",
                    "label",
                    "max",
                    "shield",
                    "showNumbers",
                    "size",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "hero-organism",
                [
                    "activeFilters",
                    "children",
                    "className",
                    "entity",
                    "error",
                    "isLoading",
                    "page",
                    "pageSize",
                    "searchValue",
                    "selectedIds",
                    "sortBy",
                    "sortDirection",
                    "totalCount",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "hero-section",
                [
                    "align",
                    "background",
                    "backgroundElement",
                    "children",
                    "className",
                    "image",
                    "imagePosition",
                    "installCommand",
                    "primaryAction",
                    "secondaryAction",
                    "subtitle",
                    "tag",
                    "tagVariant",
                    "title",
                    "titleAccent",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "icon",
                [
                    "animation",
                    "className",
                    "color",
                    "icon",
                    "name",
                    "size",
                    "strokeWidth",
                    "style",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "infinite-scroll-sentinel",
                [
                    "className",
                    "hasMore",
                    "isLoading",
                    "loadMoreEvent",
                    "loadMorePayload",
                    "threshold",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "input",
                [
                    "action",
                    "className",
                    "clearable",
                    "disabled",
                    "error",
                    "helperText",
                    "icon",
                    "inputType",
                    "label",
                    "leftIcon",
                    "onChange",
                    "onClear",
                    "options",
                    "placeholder",
                    "rightIcon",
                    "rows",
                    "value",
                ]
                .into_iter()
                .collect(),
            ),
            ("input-group", ["className", "leftAddon", "rightAddon"].into_iter().collect()),
            ("install-box", ["className", "command", "label"].into_iter().collect()),
            (
                "inventory-grid",
                [
                    "className",
                    "columns",
                    "items",
                    "onSelect",
                    "selectEvent",
                    "selectedId",
                    "size",
                    "totalSlots",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "inventory-panel",
                [
                    "className",
                    "columns",
                    "dropItemEvent",
                    "items",
                    "onDropItem",
                    "onSelectSlot",
                    "onUseItem",
                    "selectSlotEvent",
                    "selectedSlot",
                    "showTooltips",
                    "slotSize",
                    "slots",
                    "useItemEvent",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "isometric-canvas",
                [
                    "assetBaseUrl",
                    "assetManifest",
                    "attackTargets",
                    "backgroundImage",
                    "boardHeight",
                    "boardWidth",
                    "className",
                    "debug",
                    "diamondTopY",
                    "effectSpriteUrls",
                    "enableCamera",
                    "error",
                    "features",
                    "getFeatureSprite",
                    "getTerrainSprite",
                    "getUnitSprite",
                    "hasActiveEffects",
                    "hoveredTile",
                    "isLoading",
                    "onDrawEffects",
                    "onTileClick",
                    "onTileHover",
                    "onTileLeave",
                    "onUnitClick",
                    "resolveUnitFrame",
                    "scale",
                    "selectedUnitId",
                    "showMinimap",
                    "tileClickEvent",
                    "tileHoverEvent",
                    "tileLeaveEvent",
                    "tiles",
                    "unitClickEvent",
                    "unitScale",
                    "units",
                    "validMoves",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "item-slot",
                [
                    "className",
                    "empty",
                    "icon",
                    "label",
                    "onClick",
                    "quantity",
                    "rarity",
                    "selected",
                    "size",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "jazari-state-machine",
                [
                    "className",
                    "direction",
                    "entityFields",
                    "error",
                    "isLoading",
                    "schema",
                    "trait",
                    "traitIndex",
                ]
                .into_iter()
                .collect(),
            ),
            ("label", ["className", "htmlFor", "required", "text"].into_iter().collect()),
            ("landing-page-template", ["className", "entity", "featureColumns", "variant"].into_iter().collect()),
            ("law-reference-tooltip", ["children", "className", "position", "reference"].into_iter().collect()),
            (
                "lightbox",
                [
                    "className",
                    "closeAction",
                    "currentIndex",
                    "images",
                    "isOpen",
                    "onClose",
                    "onIndexChange",
                    "showCounter",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "likert-scale",
                [
                    "changeEvent",
                    "className",
                    "disabled",
                    "onChange",
                    "options",
                    "question",
                    "size",
                    "value",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "line-chart",
                [
                    "areaColor",
                    "className",
                    "data",
                    "height",
                    "lineColor",
                    "showArea",
                    "showGrid",
                    "showValues",
                    "width",
                ]
                .into_iter()
                .collect(),
            ),
            ("loading-state", ["className", "message", "title"].into_iter().collect()),
            (
                "map-view",
                [
                    "centerLat",
                    "centerLng",
                    "className",
                    "height",
                    "mapClickEvent",
                    "markerClickEvent",
                    "markers",
                    "onMapClick",
                    "onMarkerClick",
                    "routes",
                    "showAttribution",
                    "showClickedPin",
                    "zoom",
                ]
                .into_iter()
                .collect(),
            ),
            ("markdown-content", ["className", "content", "direction"].into_iter().collect()),
            ("marketing-footer", ["className", "columns", "copyright", "logo"].into_iter().collect()),
            (
                "master-detail",
                [
                    "activeFilters",
                    "className",
                    "detailFields",
                    "entity",
                    "error",
                    "isLoading",
                    "loading",
                    "masterFields",
                    "page",
                    "pageSize",
                    "searchValue",
                    "selectedIds",
                    "sortBy",
                    "sortDirection",
                    "totalCount",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "master-detail-layout",
                [
                    "className",
                    "detail",
                    "detailClassName",
                    "emptyDetail",
                    "hasSelection",
                    "master",
                    "masterClassName",
                    "masterWidth",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "matrix-question",
                [
                    "changeEvent",
                    "className",
                    "columns",
                    "disabled",
                    "onChange",
                    "rows",
                    "size",
                    "title",
                    "values",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "media-gallery",
                [
                    "actions",
                    "activeFilters",
                    "aspectRatio",
                    "className",
                    "columns",
                    "entity",
                    "error",
                    "isLoading",
                    "items",
                    "page",
                    "pageSize",
                    "searchValue",
                    "selectable",
                    "selectedIds",
                    "selectedItems",
                    "selectionEvent",
                    "showUpload",
                    "sortBy",
                    "sortDirection",
                    "title",
                    "totalCount",
                ]
                .into_iter()
                .collect(),
            ),
            ("menu", ["className", "footer", "header", "items", "position", "trigger"].into_iter().collect()),
            (
                "meter",
                [
                    "actions",
                    "className",
                    "error",
                    "isLoading",
                    "label",
                    "max",
                    "min",
                    "segments",
                    "showValue",
                    "size",
                    "thresholds",
                    "unit",
                    "value",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "mini-map",
                [
                    "className",
                    "height",
                    "mapHeight",
                    "mapWidth",
                    "tiles",
                    "units",
                    "viewportRect",
                    "width",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "modal",
                [
                    "children",
                    "className",
                    "closeEvent",
                    "closeOnEscape",
                    "closeOnOverlayClick",
                    "footer",
                    "isOpen",
                    "look",
                    "onClose",
                    "showCloseButton",
                    "size",
                    "swipeDownToClose",
                    "title",
                ]
                .into_iter()
                .collect(),
            ),
            ("modal-slot", ["children", "className", "entity", "error", "isLoading", "size", "title"].into_iter().collect()),
            ("module-card", ["data"].into_iter().collect()),
            ("navigation", ["className", "error", "isLoading", "items", "orientation"].into_iter().collect()),
            (
                "negotiator-board",
                [
                    "activeFilters",
                    "className",
                    "completeEvent",
                    "entity",
                    "error",
                    "isLoading",
                    "page",
                    "pageSize",
                    "searchValue",
                    "selectedIds",
                    "sortBy",
                    "sortDirection",
                    "totalCount",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "notification",
                [
                    "actionEvent",
                    "actionLabel",
                    "badge",
                    "className",
                    "dismissEvent",
                    "dismissible",
                    "duration",
                    "message",
                    "onAction",
                    "onDismiss",
                    "title",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "number-stepper",
                [
                    "action",
                    "actionPayload",
                    "className",
                    "disabled",
                    "label",
                    "max",
                    "min",
                    "onChange",
                    "size",
                    "step",
                    "value",
                ]
                .into_iter()
                .collect(),
            ),
            ("object-rule-panel", ["className", "disabled", "object", "onRulesChange"].into_iter().collect()),
            (
                "option-constraint-group",
                [
                    "changeEvent",
                    "className",
                    "constraint",
                    "description",
                    "groupId",
                    "onChange",
                    "options",
                    "selected",
                    "size",
                    "title",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "orbital-visualization",
                [
                    "animated",
                    "className",
                    "complexity",
                    "entity",
                    "error",
                    "isLoading",
                    "onClick",
                    "schema",
                    "showLabel",
                    "size",
                ]
                .into_iter()
                .collect(),
            ),
            ("overlay", ["action", "blur", "className", "isVisible", "onClick"].into_iter().collect()),
            (
                "page-header",
                [
                    "actions",
                    "activeTab",
                    "backEvent",
                    "breadcrumbs",
                    "children",
                    "className",
                    "error",
                    "isLoading",
                    "onTabChange",
                    "showBack",
                    "status",
                    "subtitle",
                    "tabs",
                    "title",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "pagination",
                [
                    "className",
                    "currentPage",
                    "maxVisiblePages",
                    "onPageChange",
                    "onPageSizeChange",
                    "pageChangeEvent",
                    "pageSize",
                    "pageSizeChangeEvent",
                    "pageSizeOptions",
                    "showJumpToPage",
                    "showPageSize",
                    "showTotal",
                    "totalItems",
                    "totalPages",
                ]
                .into_iter()
                .collect(),
            ),
            ("pattern-tile", ["className", "color", "size", "strokeWidth", "variant"].into_iter().collect()),
            (
                "platformer-canvas",
                [
                    "assetBaseUrl",
                    "backgroundImage",
                    "bgColor",
                    "canvasHeight",
                    "canvasWidth",
                    "className",
                    "followCamera",
                    "jumpEvent",
                    "leftEvent",
                    "platforms",
                    "player",
                    "playerSprite",
                    "rightEvent",
                    "stopEvent",
                    "tileSprites",
                    "worldHeight",
                    "worldWidth",
                ]
                .into_iter()
                .collect(),
            ),
            ("popover", ["children", "className", "content", "position", "showArrow", "trigger"].into_iter().collect()),
            (
                "positioned-canvas",
                [
                    "className",
                    "editable",
                    "height",
                    "items",
                    "moveEvent",
                    "onMove",
                    "onSelect",
                    "selectEvent",
                    "selectedId",
                    "width",
                ]
                .into_iter()
                .collect(),
            ),
            ("powerup-slots", ["active", "className", "maxSlots"].into_iter().collect()),
            (
                "pricing-card",
                [
                    "action",
                    "badge",
                    "className",
                    "description",
                    "features",
                    "highlighted",
                    "name",
                    "price",
                ]
                .into_iter()
                .collect(),
            ),
            ("pricing-grid", ["className", "plans"].into_iter().collect()),
            (
                "pricing-organism",
                [
                    "activeFilters",
                    "className",
                    "entity",
                    "error",
                    "heading",
                    "isLoading",
                    "page",
                    "pageSize",
                    "searchValue",
                    "selectedIds",
                    "sortBy",
                    "sortDirection",
                    "subtitle",
                    "totalCount",
                ]
                .into_iter()
                .collect(),
            ),
            ("pricing-page-template", ["className", "entity"].into_iter().collect()),
            (
                "progress-bar",
                [
                    "className",
                    "color",
                    "label",
                    "max",
                    "progressType",
                    "showLabel",
                    "showPercentage",
                    "size",
                    "steps",
                    "value",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            ("progress-dots", ["className", "count", "currentIndex", "getState", "onDotClick", "size"].into_iter().collect()),
            ("pull-quote", ["children", "className"].into_iter().collect()),
            ("pull-to-refresh", ["children", "className", "refreshEvent", "refreshPayload", "threshold"].into_iter().collect()),
            (
                "qr-scanner",
                [
                    "className",
                    "facingMode",
                    "fallback",
                    "onError",
                    "onScan",
                    "paused",
                    "scanEvent",
                    "showCameraControls",
                    "showOverlay",
                ]
                .into_iter()
                .collect(),
            ),
            ("quest-tracker", ["activeQuestId", "className", "quests"].into_iter().collect()),
            ("quiz-block", ["answer", "className", "question"].into_iter().collect()),
            (
                "radio",
                [
                    "action",
                    "className",
                    "error",
                    "helperText",
                    "label",
                    "options",
                    "size",
                    "value",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "range-slider",
                [
                    "action",
                    "actionPayload",
                    "buffered",
                    "className",
                    "disabled",
                    "formatValue",
                    "max",
                    "min",
                    "onChange",
                    "showTicks",
                    "showTooltip",
                    "size",
                    "step",
                    "value",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "relation-select",
                [
                    "className",
                    "clearable",
                    "disabled",
                    "emptyMessage",
                    "error",
                    "isLoading",
                    "name",
                    "onChange",
                    "options",
                    "placeholder",
                    "required",
                    "searchPlaceholder",
                    "value",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "repeatable-form-section",
                [
                    "addLabel",
                    "allowReorder",
                    "className",
                    "currentState",
                    "emptyMessage",
                    "items",
                    "maxItems",
                    "minItems",
                    "onAdd",
                    "onRemove",
                    "onReorder",
                    "readOnly",
                    "renderItem",
                    "sectionType",
                    "showAuditInfo",
                    "title",
                    "trackAddedInState",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "reply-tree",
                [
                    "className",
                    "continueThreadEvent",
                    "flagEvent",
                    "maxDepth",
                    "nodes",
                    "onContinueThread",
                    "onFlag",
                    "onReply",
                    "onVote",
                    "replyEvent",
                    "showActions",
                    "voteEvent",
                ]
                .into_iter()
                .collect(),
            ),
            ("resource-bar", ["className", "resources", "size"].into_iter().collect()),
            ("resource-counter", ["className", "color", "icon", "label", "max", "size", "value"].into_iter().collect()),
            (
                "rich-block-editor",
                [
                    "changeEvent",
                    "className",
                    "enableBlocks",
                    "initialBlocks",
                    "onChange",
                    "placeholder",
                    "readOnly",
                    "showToolbar",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "rule-editor",
                [
                    "availableActions",
                    "availableEvents",
                    "className",
                    "disabled",
                    "onChange",
                    "onRemove",
                    "rule",
                ]
                .into_iter()
                .collect(),
            ),
            ("runtime-debugger", ["className", "defaultCollapsed", "defaultTab", "mode", "position", "schema"].into_iter().collect()),
            ("scaled-diagram", ["children", "className"].into_iter().collect()),
            ("score-board", ["className", "combo", "highScore", "level", "multiplier", "score"].into_iter().collect()),
            ("score-display", ["animated", "className", "icon", "label", "locale", "size", "value"].into_iter().collect()),
            (
                "search-input",
                [
                    "className",
                    "clearable",
                    "debounceMs",
                    "entity",
                    "event",
                    "isLoading",
                    "onSearch",
                    "placeholder",
                    "query",
                    "value",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "section",
                [
                    "action",
                    "as",
                    "children",
                    "className",
                    "contentClassName",
                    "description",
                    "divider",
                    "entity",
                    "error",
                    "headerClassName",
                    "isLoading",
                    "padding",
                    "title",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            ("section-header", ["align", "className", "level", "subtitle", "title"].into_iter().collect()),
            (
                "select",
                [
                    "action",
                    "className",
                    "clearable",
                    "error",
                    "groups",
                    "multiple",
                    "onChange",
                    "options",
                    "placeholder",
                    "searchable",
                    "value",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "sequence-bar",
                [
                    "categoryColors",
                    "className",
                    "currentStep",
                    "maxSlots",
                    "onSlotDrop",
                    "onSlotRemove",
                    "playing",
                    "size",
                    "slotFeedback",
                    "slots",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "sequencer-board",
                [
                    "activeFilters",
                    "categoryColors",
                    "className",
                    "completeEvent",
                    "entity",
                    "error",
                    "isLoading",
                    "page",
                    "pageSize",
                    "playEvent",
                    "searchValue",
                    "selectedIds",
                    "sortBy",
                    "sortDirection",
                    "stepDurationMs",
                    "totalCount",
                ]
                .into_iter()
                .collect(),
            ),
            ("service-catalog", ["className", "services"].into_iter().collect()),
            ("showcase-card", ["accentColor", "badge", "className", "description", "href", "image", "title"].into_iter().collect()),
            (
                "showcase-organism",
                [
                    "activeFilters",
                    "className",
                    "columns",
                    "entity",
                    "error",
                    "heading",
                    "isLoading",
                    "page",
                    "pageSize",
                    "searchValue",
                    "selectedIds",
                    "sortBy",
                    "sortDirection",
                    "subtitle",
                    "totalCount",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "side-panel",
                [
                    "children",
                    "className",
                    "closeEvent",
                    "isOpen",
                    "onClose",
                    "position",
                    "showOverlay",
                    "title",
                    "width",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "sidebar",
                [
                    "brandName",
                    "className",
                    "closeEvent",
                    "collapseChangeEvent",
                    "collapsed",
                    "defaultCollapsed",
                    "error",
                    "footerContent",
                    "hideCollapseButton",
                    "isLoading",
                    "items",
                    "logo",
                    "logoClickEvent",
                    "logoSrc",
                    "showCloseButton",
                    "userSection",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "signature-pad",
                [
                    "className",
                    "clearEvent",
                    "entity",
                    "error",
                    "height",
                    "helperText",
                    "isLoading",
                    "label",
                    "onChange",
                    "readOnly",
                    "signEvent",
                    "strokeColor",
                    "strokeWidth",
                    "value",
                ]
                .into_iter()
                .collect(),
            ),
            ("simple-grid", ["children", "className", "cols", "gap", "maxCols", "minChildWidth"].into_iter().collect()),
            ("simulation-canvas", ["className", "height", "preset", "running", "speed", "width"].into_iter().collect()),
            (
                "simulation-controls",
                [
                    "className",
                    "onParameterChange",
                    "onPause",
                    "onPlay",
                    "onReset",
                    "onSpeedChange",
                    "onStep",
                    "parameters",
                    "running",
                    "speed",
                ]
                .into_iter()
                .collect(),
            ),
            ("simulation-graph", ["className", "color", "data", "height", "label", "maxPoints", "unit", "width"].into_iter().collect()),
            (
                "simulator-board",
                [
                    "activeFilters",
                    "className",
                    "completeEvent",
                    "entity",
                    "error",
                    "isLoading",
                    "page",
                    "pageSize",
                    "searchValue",
                    "selectedIds",
                    "sortBy",
                    "sortDirection",
                    "totalCount",
                ]
                .into_iter()
                .collect(),
            ),
            ("skeleton", ["className", "columns", "fields", "rows", "variant"].into_iter().collect()),
            ("social-proof", ["className", "items", "variant"].into_iter().collect()),
            (
                "sortable-list",
                [
                    "className",
                    "dragHandlePosition",
                    "items",
                    "renderItem",
                    "reorderEvent",
                    "reorderPayload",
                ]
                .into_iter()
                .collect(),
            ),
            ("spacer", ["axis", "className", "size"].into_iter().collect()),
            ("sparkline", ["className", "color", "data", "fill", "height", "strokeWidth", "width"].into_iter().collect()),
            ("spinner", ["className", "overlay", "size"].into_iter().collect()),
            (
                "split",
                [
                    "align",
                    "children",
                    "className",
                    "entity",
                    "error",
                    "gap",
                    "isLoading",
                    "leftClassName",
                    "ratio",
                    "reverse",
                    "rightClassName",
                    "stackBreakpoint",
                    "stackOnMobile",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "split-pane",
                [
                    "className",
                    "direction",
                    "left",
                    "leftClassName",
                    "minSize",
                    "ratio",
                    "resizable",
                    "right",
                    "rightClassName",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "split-section",
                [
                    "background",
                    "bullets",
                    "children",
                    "className",
                    "description",
                    "image",
                    "imagePosition",
                    "title",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "sprite",
                [
                    "action",
                    "className",
                    "columns",
                    "flipX",
                    "flipY",
                    "frame",
                    "frameHeight",
                    "frameWidth",
                    "onClick",
                    "opacity",
                    "rotation",
                    "scale",
                    "spritesheet",
                    "x",
                    "y",
                    "zIndex",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "stack",
                [
                    "action",
                    "actionPayload",
                    "align",
                    "as",
                    "children",
                    "className",
                    "direction",
                    "flex",
                    "gap",
                    "justify",
                    "onClick",
                    "onKeyDown",
                    "responsive",
                    "reverse",
                    "role",
                    "style",
                    "tabIndex",
                    "wrap",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "star-rating",
                [
                    "action",
                    "actionPayload",
                    "className",
                    "label",
                    "max",
                    "onChange",
                    "precision",
                    "readOnly",
                    "size",
                    "value",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "stat-badge",
                [
                    "className",
                    "field",
                    "format",
                    "icon",
                    "label",
                    "max",
                    "size",
                    "source",
                    "value",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            ("stat-card", ["className", "label", "size", "value"].into_iter().collect()),
            (
                "stat-display",
                [
                    "className",
                    "clickEvent",
                    "compact",
                    "error",
                    "format",
                    "icon",
                    "iconBg",
                    "iconColor",
                    "isLoading",
                    "label",
                    "look",
                    "max",
                    "prefix",
                    "size",
                    "sparklineData",
                    "suffix",
                    "target",
                    "trend",
                    "trendFormat",
                    "trendPolarity",
                    "value",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "state-architect-board",
                [
                    "activeFilters",
                    "className",
                    "completeEvent",
                    "entity",
                    "error",
                    "isLoading",
                    "page",
                    "pageSize",
                    "searchValue",
                    "selectedIds",
                    "sortBy",
                    "sortDirection",
                    "stepDurationMs",
                    "testEvent",
                    "totalCount",
                ]
                .into_iter()
                .collect(),
            ),
            ("state-indicator", ["animated", "className", "label", "size", "state", "stateStyles"].into_iter().collect()),
            ("state-machine-view", ["className", "error", "isLoading", "layoutData", "renderStateNode"].into_iter().collect()),
            (
                "state-node",
                [
                    "className",
                    "isCurrent",
                    "isInitial",
                    "isSelected",
                    "name",
                    "onClick",
                    "position",
                ]
                .into_iter()
                .collect(),
            ),
            ("stats-grid", ["className", "columns", "stats"].into_iter().collect()),
            (
                "stats-organism",
                [
                    "activeFilters",
                    "className",
                    "columns",
                    "entity",
                    "error",
                    "isLoading",
                    "page",
                    "pageSize",
                    "searchValue",
                    "selectedIds",
                    "sortBy",
                    "sortDirection",
                    "totalCount",
                ]
                .into_iter()
                .collect(),
            ),
            ("status-dot", ["className", "label", "pulse", "size", "status"].into_iter().collect()),
            ("status-effect", ["className", "duration", "icon", "label", "size", "stacks", "variant"].into_iter().collect()),
            ("step-flow", ["className", "orientation", "showConnectors", "steps"].into_iter().collect()),
            (
                "step-flow-organism",
                [
                    "activeFilters",
                    "className",
                    "entity",
                    "error",
                    "heading",
                    "isLoading",
                    "orientation",
                    "page",
                    "pageSize",
                    "searchValue",
                    "selectedIds",
                    "showConnectors",
                    "sortBy",
                    "sortDirection",
                    "subtitle",
                    "totalCount",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "svg-branch",
                [
                    "asRoot",
                    "branches",
                    "className",
                    "color",
                    "height",
                    "opacity",
                    "size",
                    "variant",
                    "width",
                    "x",
                    "y",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "svg-connection",
                [
                    "asRoot",
                    "className",
                    "color",
                    "height",
                    "opacity",
                    "strokeWidth",
                    "variant",
                    "width",
                    "x1",
                    "x2",
                    "y1",
                    "y2",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "svg-flow",
                [
                    "animated",
                    "asRoot",
                    "className",
                    "color",
                    "height",
                    "opacity",
                    "points",
                    "strokeWidth",
                    "width",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "svg-grid",
                [
                    "asRoot",
                    "className",
                    "color",
                    "cols",
                    "height",
                    "highlights",
                    "nodeRadius",
                    "opacity",
                    "rows",
                    "spacing",
                    "width",
                    "x",
                    "y",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "svg-lobe",
                [
                    "asRoot",
                    "className",
                    "color",
                    "cx",
                    "cy",
                    "height",
                    "opacity",
                    "rotation",
                    "rx",
                    "ry",
                    "shells",
                    "width",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "svg-mesh",
                [
                    "asRoot",
                    "className",
                    "color",
                    "connectionDensity",
                    "cx",
                    "cy",
                    "height",
                    "nodes",
                    "opacity",
                    "radius",
                    "width",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "svg-morph",
                [
                    "asRoot",
                    "className",
                    "color",
                    "height",
                    "opacity",
                    "size",
                    "variant",
                    "width",
                    "x",
                    "y",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "svg-node",
                [
                    "asRoot",
                    "className",
                    "color",
                    "height",
                    "label",
                    "opacity",
                    "r",
                    "variant",
                    "width",
                    "x",
                    "y",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "svg-pulse",
                [
                    "animated",
                    "asRoot",
                    "className",
                    "color",
                    "cx",
                    "cy",
                    "height",
                    "maxRadius",
                    "opacity",
                    "rings",
                    "width",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "svg-ring",
                [
                    "asRoot",
                    "className",
                    "color",
                    "cx",
                    "cy",
                    "height",
                    "label",
                    "opacity",
                    "r",
                    "strokeWidth",
                    "variant",
                    "width",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "svg-shield",
                [
                    "asRoot",
                    "className",
                    "color",
                    "height",
                    "opacity",
                    "size",
                    "variant",
                    "width",
                    "x",
                    "y",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "svg-stack",
                [
                    "asRoot",
                    "className",
                    "color",
                    "height",
                    "labels",
                    "layers",
                    "opacity",
                    "svgHeight",
                    "svgWidth",
                    "width",
                    "x",
                    "y",
                ]
                .into_iter()
                .collect(),
            ),
            ("swipeable-row", ["children", "className", "itemData", "leftActions", "rightActions", "threshold"].into_iter().collect()),
            (
                "switch",
                [
                    "checked",
                    "className",
                    "defaultChecked",
                    "disabled",
                    "id",
                    "label",
                    "name",
                    "onChange",
                ]
                .into_iter()
                .collect(),
            ),
            ("tabbed-container", ["activeTab", "className", "defaultTab", "onTabChange", "position", "tabs"].into_iter().collect()),
            (
                "table-view",
                [
                    "accepts",
                    "children",
                    "className",
                    "columns",
                    "dndItemIdField",
                    "dndRoot",
                    "dragGroup",
                    "dropEvent",
                    "emptyMessage",
                    "entity",
                    "error",
                    "fields",
                    "groupBy",
                    "isLoading",
                    "itemActions",
                    "look",
                    "maxInlineActions",
                    "pageSize",
                    "positionEvent",
                    "renderItem",
                    "reorderEvent",
                    "selectEvent",
                    "selectable",
                    "selectedIds",
                    "sortColumn",
                    "sortDirection",
                    "sortEvent",
                    "sortable",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "tabs",
                [
                    "activeTab",
                    "className",
                    "defaultActiveTab",
                    "items",
                    "onTabChange",
                    "orientation",
                    "tabChangeEvent",
                    "tabs",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            ("tag-cloud", ["className", "tags", "variant"].into_iter().collect()),
            (
                "tag-input",
                [
                    "addEvent",
                    "className",
                    "disabled",
                    "helperText",
                    "onChange",
                    "placeholder",
                    "removeEvent",
                    "unique",
                    "value",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            ("team-card", ["avatar", "bio", "className", "name", "nameAr", "role"].into_iter().collect()),
            (
                "team-organism",
                [
                    "activeFilters",
                    "className",
                    "entity",
                    "error",
                    "heading",
                    "isLoading",
                    "page",
                    "pageSize",
                    "searchValue",
                    "selectedIds",
                    "sortBy",
                    "sortDirection",
                    "subtitle",
                    "totalCount",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "text-highlight",
                [
                    "action",
                    "annotationId",
                    "children",
                    "className",
                    "highlightType",
                    "hoverEvent",
                    "isActive",
                    "onClick",
                    "onMouseEnter",
                    "onMouseLeave",
                ]
                .into_iter()
                .collect(),
            ),
            ("textarea", ["action", "className", "error", "onChange", "placeholder", "rows"].into_iter().collect()),
            ("theme-toggle", ["className", "showLabel", "size"].into_iter().collect()),
            ("time-slot-cell", ["children", "className", "isOccupied", "onClick", "time"].into_iter().collect()),
            (
                "timeline",
                [
                    "className",
                    "entity",
                    "error",
                    "fields",
                    "isLoading",
                    "itemActions",
                    "items",
                    "look",
                    "title",
                ]
                .into_iter()
                .collect(),
            ),
            ("timer-display", ["className", "format", "lowThreshold", "running", "seconds", "size"].into_iter().collect()),
            (
                "toast-slot",
                [
                    "children",
                    "className",
                    "duration",
                    "entity",
                    "error",
                    "isLoading",
                    "title",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "tooltip",
                [
                    "children",
                    "className",
                    "content",
                    "delay",
                    "hideDelay",
                    "position",
                    "showArrow",
                ]
                .into_iter()
                .collect(),
            ),
            ("trait-frame", ["fallback", "traitName"].into_iter().collect()),
            (
                "trait-slot",
                [
                    "categoryColors",
                    "className",
                    "clickEvent",
                    "draggable",
                    "entity",
                    "equippedItem",
                    "error",
                    "feedback",
                    "isLoading",
                    "lockLabel",
                    "locked",
                    "onClick",
                    "onDragStart",
                    "onItemDrop",
                    "onRemove",
                    "removeEvent",
                    "selected",
                    "showTooltip",
                    "size",
                    "slotNumber",
                    "tooltipFrameUrl",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "trait-state-viewer",
                [
                    "className",
                    "entity",
                    "error",
                    "isLoading",
                    "onStateClick",
                    "showTransitions",
                    "size",
                    "stateStyles",
                    "trait",
                    "variant",
                ]
                .into_iter()
                .collect(),
            ),
            ("transition-arrow", ["className", "eventLabel", "from", "guardHint", "isActive", "onClick", "to"].into_iter().collect()),
            ("trend-indicator", ["className", "direction", "invert", "label", "showValue", "size", "value"].into_iter().collect()),
            ("turn-indicator", ["activeTeam", "className", "currentTurn", "maxTurns", "phase", "size"].into_iter().collect()),
            ("turn-panel", ["actions", "activeTeam", "className", "currentTurn", "maxTurns", "phase"].into_iter().collect()),
            ("typewriter-text", ["className", "onComplete", "speed", "startDelay", "text"].into_iter().collect()),
            (
                "typography",
                [
                    "align",
                    "as",
                    "children",
                    "className",
                    "color",
                    "content",
                    "id",
                    "level",
                    "overflow",
                    "size",
                    "style",
                    "truncate",
                    "variant",
                    "weight",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "ui-slot-renderer",
                [
                    "className",
                    "entity",
                    "error",
                    "hudMode",
                    "includeFloating",
                    "includeHud",
                    "isLoading",
                    "suspense",
                ]
                .into_iter()
                .collect(),
            ),
            ("uncontrolled-battle-board", ["assetManifest", "entity", "features", "tiles", "units"].into_iter().collect()),
            ("unit-command-bar", ["className", "commands", "selectedUnitId"].into_iter().collect()),
            (
                "upload-drop-zone",
                [
                    "accept",
                    "action",
                    "actionPayload",
                    "className",
                    "description",
                    "disabled",
                    "icon",
                    "label",
                    "maxFiles",
                    "maxSize",
                    "onFiles",
                ]
                .into_iter()
                .collect(),
            ),
            ("variable-panel", ["className", "entityName", "variables"].into_iter().collect()),
            (
                "version-diff",
                [
                    "afterId",
                    "beforeId",
                    "className",
                    "language",
                    "onRevert",
                    "onSelectAfter",
                    "onSelectBefore",
                    "revertEvent",
                    "revisions",
                    "selectAfterEvent",
                    "selectBeforeEvent",
                    "view",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "violation-alert",
                [
                    "className",
                    "compact",
                    "dismissible",
                    "onDismiss",
                    "onNavigateToField",
                    "severity",
                    "violation",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "vote-stack",
                [
                    "className",
                    "count",
                    "disabled",
                    "label",
                    "onVote",
                    "size",
                    "userVote",
                    "variant",
                    "voteEvent",
                ]
                .into_iter()
                .collect(),
            ),
            ("waypoint-marker", ["active", "className", "completed", "icon", "label", "size"].into_iter().collect()),
            (
                "wizard-container",
                [
                    "allowBack",
                    "className",
                    "compact",
                    "currentStep",
                    "entity",
                    "error",
                    "isLoading",
                    "onComplete",
                    "onStepChange",
                    "showProgress",
                    "steps",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "wizard-navigation",
                [
                    "backLabel",
                    "className",
                    "compact",
                    "completeLabel",
                    "currentStep",
                    "isValid",
                    "nextLabel",
                    "onBack",
                    "onBackClick",
                    "onComplete",
                    "onCompleteClick",
                    "onNext",
                    "onNextClick",
                    "showBack",
                    "showComplete",
                    "showNext",
                    "totalSteps",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "wizard-progress",
                [
                    "allowNavigation",
                    "className",
                    "compact",
                    "currentStep",
                    "onStepClick",
                    "stepClickEvent",
                    "steps",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "world-map-board",
                [
                    "allowMoveAllHeroes",
                    "assetManifest",
                    "battleEncounterEvent",
                    "className",
                    "diamondTopY",
                    "effectSpriteUrls",
                    "enableCamera",
                    "entity",
                    "error",
                    "featureEnterEvent",
                    "features",
                    "footer",
                    "header",
                    "heroMoveEvent",
                    "heroSelectEvent",
                    "isInRange",
                    "isLoading",
                    "onBattleEncounter",
                    "onFeatureEnter",
                    "onHeroMove",
                    "onHeroSelect",
                    "overlay",
                    "resolveUnitFrame",
                    "scale",
                    "sidePanel",
                    "tileClickEvent",
                    "tiles",
                    "unitScale",
                    "units",
                ]
                .into_iter()
                .collect(),
            ),
            (
                "world-map-template",
                [
                    "allowMoveAllHeroes",
                    "assetManifest",
                    "className",
                    "diamondTopY",
                    "entity",
                    "features",
                    "scale",
                    "tiles",
                    "unitScale",
                    "units",
                ]
                .into_iter()
                .collect(),
            ),
            ("xp-bar", ["animated", "className", "current", "level", "max", "showLabel", "size"].into_iter().collect()),
        ]
        .into_iter()
        .collect()
    });

/// Prop types per pattern (pattern_type -> prop_name -> allowed_types)
/// Only includes props with concrete types (not "unknown").
pub static PATTERN_PROP_TYPES: LazyLock<HashMap<&'static str, HashMap<&'static str, &'static [&'static str]>>> =
    LazyLock::new(|| {
        [
            ("about-page-template", [("entity", &["object"] as &[&str]), ("className", &["string"] as &[&str])].into_iter().collect()),
            (
                "accordion",
                [
                    ("items", &["array"] as &[&str]),
                    ("multiple", &["boolean"] as &[&str]),
                    ("defaultOpenItems", &["array"] as &[&str]),
                    ("defaultOpen", &["array"] as &[&str]),
                    ("openItems", &["array"] as &[&str]),
                    ("onItemToggle", &["function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("toggleEvent", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "action-button",
                [
                    ("label", &["string"] as &[&str]),
                    ("icon", &["icon", "string"] as &[&str]),
                    ("cooldown", &["number"] as &[&str]),
                    ("disabled", &["boolean"] as &[&str]),
                    ("hotkey", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("onClick", &["function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "action-buttons",
                [
                    ("buttons", &["array"] as &[&str]),
                    ("onAction", &["function"] as &[&str]),
                    ("actionEvent", &["string"] as &[&str]),
                    ("layout", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("disabled", &["boolean"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "action-palette",
                [
                    ("actions", &["array"] as &[&str]),
                    ("usedActionIds", &["array"] as &[&str]),
                    ("allowDuplicates", &["boolean"] as &[&str]),
                    ("categoryColors", &["object"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "action-tile",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("action", &["object"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("disabled", &["boolean"] as &[&str]),
                    ("categoryColors", &["object"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "alert",
                [
                    ("children", &["node"] as &[&str]),
                    ("message", &["string"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("dismissible", &["boolean"] as &[&str]),
                    ("onDismiss", &["function"] as &[&str]),
                    ("onClose", &["function"] as &[&str]),
                    ("actions", &["node"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("dismissEvent", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "animated-counter",
                [
                    ("value", &["number", "string"] as &[&str]),
                    ("duration", &["number"] as &[&str]),
                    ("prefix", &["string"] as &[&str]),
                    ("suffix", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "animated-graphic",
                [
                    ("className", &["string"] as &[&str]),
                    ("src", &["asset"] as &[&str]),
                    ("svgContent", &["string"] as &[&str]),
                    ("animation", &["string"] as &[&str]),
                    ("animate", &["boolean"] as &[&str]),
                    ("duration", &["number"] as &[&str]),
                    ("delay", &["number"] as &[&str]),
                    ("easing", &["string"] as &[&str]),
                    ("width", &["string", "number"] as &[&str]),
                    ("height", &["string", "number"] as &[&str]),
                    ("strokeColor", &["string"] as &[&str]),
                    ("fillColor", &["string"] as &[&str]),
                    ("alt", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "animated-reveal",
                [
                    ("className", &["string"] as &[&str]),
                    ("trigger", &["string"] as &[&str]),
                    ("animation", &["string"] as &[&str]),
                    ("duration", &["number"] as &[&str]),
                    ("delay", &["number"] as &[&str]),
                    ("threshold", &["number"] as &[&str]),
                    ("once", &["boolean"] as &[&str]),
                    ("animate", &["boolean"] as &[&str]),
                    ("easing", &["string"] as &[&str]),
                    ("children", &["node", "function"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "article-section",
                [
                    ("title", &["string"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("maxWidth", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("aside", [("className", &["string"] as &[&str]), ("children", &["node"] as &[&str])].into_iter().collect()),
            (
                "auth-layout",
                [
                    ("appName", &["string"] as &[&str]),
                    ("logo", &["node"] as &[&str]),
                    ("backgroundImage", &["asset"] as &[&str]),
                    ("showBranding", &["boolean"] as &[&str]),
                    ("brandingContent", &["node"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "avatar",
                [
                    ("src", &["asset"] as &[&str]),
                    ("alt", &["string"] as &[&str]),
                    ("name", &["string"] as &[&str]),
                    ("initials", &["string"] as &[&str]),
                    ("icon", &["icon", "string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("status", &["string"] as &[&str]),
                    ("badge", &["string", "number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("onClick", &["function"] as &[&str]),
                    ("action", &["string"] as &[&str]),
                    ("actionPayload", &["object"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "badge",
                [
                    ("className", &["string"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("amount", &["number"] as &[&str]),
                    ("label", &["string", "number"] as &[&str]),
                    ("icon", &["icon", "string"] as &[&str]),
                    ("onRemove", &["function"] as &[&str]),
                    ("removeLabel", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "battle-board",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("tiles", &["array"] as &[&str]),
                    ("units", &["array"] as &[&str]),
                    ("features", &["array"] as &[&str]),
                    ("assetManifest", &["object"] as &[&str]),
                    ("scale", &["number"] as &[&str]),
                    ("unitScale", &["number"] as &[&str]),
                    ("header", &["function"] as &[&str]),
                    ("sidebar", &["function"] as &[&str]),
                    ("actions", &["function"] as &[&str]),
                    ("overlay", &["function"] as &[&str]),
                    ("gameOverOverlay", &["function"] as &[&str]),
                    ("onAttack", &["function"] as &[&str]),
                    ("onGameEnd", &["function"] as &[&str]),
                    ("onUnitMove", &["function"] as &[&str]),
                    ("calculateDamage", &["function"] as &[&str]),
                    ("onDrawEffects", &["function"] as &[&str]),
                    ("hasActiveEffects", &["boolean"] as &[&str]),
                    ("effectSpriteUrls", &["array"] as &[&str]),
                    ("resolveUnitFrame", &["function"] as &[&str]),
                    ("tileClickEvent", &["string"] as &[&str]),
                    ("unitClickEvent", &["string"] as &[&str]),
                    ("endTurnEvent", &["string"] as &[&str]),
                    ("cancelEvent", &["string"] as &[&str]),
                    ("gameEndEvent", &["string"] as &[&str]),
                    ("playAgainEvent", &["string"] as &[&str]),
                    ("attackEvent", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "battle-template",
                [
                    ("entity", &["object"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("scale", &["number"] as &[&str]),
                    ("unitScale", &["number"] as &[&str]),
                    ("tiles", &["array"] as &[&str]),
                    ("units", &["array"] as &[&str]),
                    ("features", &["array"] as &[&str]),
                    ("assetManifest", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("behavior-view", [("data", &["object"] as &[&str])].into_iter().collect()),
            (
                "book-chapter-view",
                [
                    ("className", &["string"] as &[&str]),
                    ("chapter", &["object"] as &[&str]),
                    ("orbitalSchema", &["object"] as &[&str]),
                    ("direction", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "book-cover-page",
                [
                    ("className", &["string"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("subtitle", &["string"] as &[&str]),
                    ("author", &["string"] as &[&str]),
                    ("coverImageUrl", &["asset"] as &[&str]),
                    ("direction", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "book-nav-bar",
                [
                    ("className", &["string"] as &[&str]),
                    ("currentPage", &["number"] as &[&str]),
                    ("totalPages", &["number"] as &[&str]),
                    ("chapterTitle", &["string"] as &[&str]),
                    ("direction", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "book-table-of-contents",
                [
                    ("className", &["string"] as &[&str]),
                    ("parts", &["object", "array"] as &[&str]),
                    ("currentChapterId", &["string"] as &[&str]),
                    ("direction", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "book-viewer",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("initialPage", &["number"] as &[&str]),
                    ("fieldMap", &["object", "string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "box",
                [
                    ("className", &["string"] as &[&str]),
                    ("padding", &["string"] as &[&str]),
                    ("paddingX", &["string"] as &[&str]),
                    ("paddingY", &["string"] as &[&str]),
                    ("margin", &["string"] as &[&str]),
                    ("marginX", &["string"] as &[&str]),
                    ("marginY", &["string"] as &[&str]),
                    ("bg", &["string"] as &[&str]),
                    ("border", &["boolean"] as &[&str]),
                    ("rounded", &["string"] as &[&str]),
                    ("shadow", &["string"] as &[&str]),
                    ("display", &["string"] as &[&str]),
                    ("fullWidth", &["boolean"] as &[&str]),
                    ("fullHeight", &["boolean"] as &[&str]),
                    ("overflow", &["string"] as &[&str]),
                    ("position", &["string"] as &[&str]),
                    ("as", &["component"] as &[&str]),
                    ("action", &["string"] as &[&str]),
                    ("actionPayload", &["object"] as &[&str]),
                    ("hoverEvent", &["string"] as &[&str]),
                    ("maxWidth", &["string"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "branching-logic-builder",
                [
                    ("questions", &["array", "object"] as &[&str]),
                    ("rules", &["array", "object"] as &[&str]),
                    ("onRulesChange", &["function"] as &[&str]),
                    ("rulesChangeEvent", &["string"] as &[&str]),
                    ("readOnly", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "breadcrumb",
                [
                    ("items", &["array"] as &[&str]),
                    ("separator", &["icon", "string"] as &[&str]),
                    ("maxItems", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "builder-board",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("completeEvent", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "button",
                [
                    ("className", &["string"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("leftIcon", &["icon", "string"] as &[&str]),
                    ("rightIcon", &["icon", "string"] as &[&str]),
                    ("icon", &["icon", "string"] as &[&str]),
                    ("iconRight", &["icon", "string"] as &[&str]),
                    ("action", &["string"] as &[&str]),
                    ("actionPayload", &["object"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                    ("disabled", &["boolean"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "calendar-grid",
                [
                    ("weekStart", &["date"] as &[&str]),
                    ("timeSlots", &["array"] as &[&str]),
                    ("events", &["object", "array"] as &[&str]),
                    ("onSlotClick", &["function"] as &[&str]),
                    ("onDayClick", &["function"] as &[&str]),
                    ("onEventClick", &["function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("longPressEvent", &["string"] as &[&str]),
                    ("longPressPayload", &["object"] as &[&str]),
                    ("swipeLeftEvent", &["string"] as &[&str]),
                    ("swipeRightEvent", &["string"] as &[&str]),
                    ("dayWindow", &["number", "string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "canvas-effect",
                [
                    ("actionType", &["string"] as &[&str]),
                    ("x", &["number"] as &[&str]),
                    ("y", &["number"] as &[&str]),
                    ("duration", &["number"] as &[&str]),
                    ("intensity", &["number"] as &[&str]),
                    ("onComplete", &["function"] as &[&str]),
                    ("completeEvent", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("effectSpriteUrl", &["asset"] as &[&str]),
                    ("assetBaseUrl", &["asset"] as &[&str]),
                    ("assetManifest", &["object"] as &[&str]),
                    ("width", &["number"] as &[&str]),
                    ("height", &["number"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "card",
                [
                    ("className", &["string"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("padding", &["string"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("subtitle", &["string"] as &[&str]),
                    ("shadow", &["string"] as &[&str]),
                    ("look", &["string"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("action", &["string"] as &[&str]),
                    ("loading", &["boolean"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "carousel",
                [
                    ("items", &["array"] as &[&str]),
                    ("renderItem", &["function"] as &[&str]),
                    ("children", &["function"] as &[&str]),
                    ("autoPlay", &["boolean"] as &[&str]),
                    ("autoPlayInterval", &["number"] as &[&str]),
                    ("showDots", &["boolean"] as &[&str]),
                    ("showArrows", &["boolean"] as &[&str]),
                    ("loop", &["boolean"] as &[&str]),
                    ("slideChangeEvent", &["string"] as &[&str]),
                    ("slideChangePayload", &["object"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "case-study-card",
                [
                    ("title", &["string"] as &[&str]),
                    ("description", &["string"] as &[&str]),
                    ("category", &["string"] as &[&str]),
                    ("categoryColor", &["string"] as &[&str]),
                    ("href", &["string"] as &[&str]),
                    ("linkLabel", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "case-study-organism",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("heading", &["string"] as &[&str]),
                    ("subtitle", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "castle-board",
                [
                    ("entity", &["object", "array"] as &[&str]),
                    ("tiles", &["array"] as &[&str]),
                    ("units", &["array"] as &[&str]),
                    ("features", &["array"] as &[&str]),
                    ("assetManifest", &["object"] as &[&str]),
                    ("scale", &["number"] as &[&str]),
                    ("header", &["function"] as &[&str]),
                    ("sidePanel", &["function"] as &[&str]),
                    ("overlay", &["function"] as &[&str]),
                    ("footer", &["function"] as &[&str]),
                    ("onFeatureClick", &["function"] as &[&str]),
                    ("onUnitClick", &["function"] as &[&str]),
                    ("onTileClick", &["function"] as &[&str]),
                    ("featureClickEvent", &["string"] as &[&str]),
                    ("unitClickEvent", &["string"] as &[&str]),
                    ("tileClickEvent", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "castle-template",
                [
                    ("entity", &["object"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("scale", &["number"] as &[&str]),
                    ("tiles", &["array"] as &[&str]),
                    ("units", &["array"] as &[&str]),
                    ("features", &["array"] as &[&str]),
                    ("assetManifest", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "center",
                [
                    ("inline", &["boolean"] as &[&str]),
                    ("horizontal", &["boolean"] as &[&str]),
                    ("vertical", &["boolean"] as &[&str]),
                    ("minHeight", &["string", "number"] as &[&str]),
                    ("fullHeight", &["boolean"] as &[&str]),
                    ("fullWidth", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("style", &["object"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("as", &["component"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "chart",
                [
                    ("title", &["string"] as &[&str]),
                    ("subtitle", &["string"] as &[&str]),
                    ("chartType", &["string"] as &[&str]),
                    ("look", &["string"] as &[&str]),
                    ("series", &["array"] as &[&str]),
                    ("data", &["array"] as &[&str]),
                    ("scatterData", &["array"] as &[&str]),
                    ("height", &["number"] as &[&str]),
                    ("showLegend", &["boolean"] as &[&str]),
                    ("showValues", &["boolean"] as &[&str]),
                    ("stack", &["string"] as &[&str]),
                    ("timeAxis", &["boolean"] as &[&str]),
                    ("drillEvent", &["string"] as &[&str]),
                    ("actions", &["array"] as &[&str]),
                    ("entity", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "chart-legend",
                [
                    ("items", &["array"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("direction", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "checkbox",
                [
                    ("className", &["string"] as &[&str]),
                    ("checked", &["boolean"] as &[&str]),
                    ("defaultChecked", &["boolean"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                    ("onChange", &["function", "string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "choice-button",
                [
                    ("text", &["string"] as &[&str]),
                    ("index", &["number"] as &[&str]),
                    ("disabled", &["boolean"] as &[&str]),
                    ("selected", &["boolean"] as &[&str]),
                    ("onClick", &["function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "classifier-board",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("completeEvent", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "code-block",
                [
                    ("code", &["string"] as &[&str]),
                    ("language", &["string"] as &[&str]),
                    ("showCopyButton", &["boolean"] as &[&str]),
                    ("showLanguageBadge", &["boolean"] as &[&str]),
                    ("maxHeight", &["string", "number"] as &[&str]),
                    ("foldable", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("editable", &["boolean"] as &[&str]),
                    ("onChange", &["function"] as &[&str]),
                    ("errorLines", &["object"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("mode", &["string"] as &[&str]),
                    ("diff", &["array"] as &[&str]),
                    ("oldValue", &["string"] as &[&str]),
                    ("newValue", &["string"] as &[&str]),
                    ("showLineNumbers", &["boolean"] as &[&str]),
                    ("wordWrap", &["boolean"] as &[&str]),
                    ("files", &["array"] as &[&str]),
                    ("actions", &["array"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("showCopy", &["boolean"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "combat-log",
                [
                    ("events", &["array"] as &[&str]),
                    ("maxVisible", &["number"] as &[&str]),
                    ("autoScroll", &["boolean"] as &[&str]),
                    ("showTimestamps", &["boolean"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "combo-counter",
                [
                    ("combo", &["number"] as &[&str]),
                    ("multiplier", &["number"] as &[&str]),
                    ("streak", &["number"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "community-links",
                [
                    ("github", &["object"] as &[&str]),
                    ("discord", &["object"] as &[&str]),
                    ("twitter", &["object"] as &[&str]),
                    ("heading", &["string"] as &[&str]),
                    ("subtitle", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "conditional-wrapper",
                [
                    ("condition", &["sexpr"] as &[&str]),
                    ("context", &["object"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("fallback", &["node"] as &[&str]),
                    ("animate", &["boolean"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "confetti-effect",
                [
                    ("trigger", &["boolean"] as &[&str]),
                    ("duration", &["number"] as &[&str]),
                    ("particleCount", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "confirm-dialog",
                [
                    ("isOpen", &["boolean"] as &[&str]),
                    ("onClose", &["function"] as &[&str]),
                    ("onConfirm", &["function"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("message", &["string", "node"] as &[&str]),
                    ("description", &["string", "node"] as &[&str]),
                    ("confirmText", &["string"] as &[&str]),
                    ("confirmLabel", &["string"] as &[&str]),
                    ("cancelText", &["string"] as &[&str]),
                    ("cancelLabel", &["string"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("entity", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "container",
                [
                    ("size", &["string"] as &[&str]),
                    ("maxWidth", &["string"] as &[&str]),
                    ("padding", &["string"] as &[&str]),
                    ("center", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("as", &["component"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "content-renderer",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("content", &["string"] as &[&str]),
                    ("segments", &["array"] as &[&str]),
                    ("direction", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "content-section",
                [
                    ("children", &["node"] as &[&str]),
                    ("background", &["string"] as &[&str]),
                    ("padding", &["string"] as &[&str]),
                    ("id", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "control-button",
                [
                    ("label", &["string"] as &[&str]),
                    ("icon", &["icon", "string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("shape", &["string"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("onPress", &["function"] as &[&str]),
                    ("onRelease", &["function"] as &[&str]),
                    ("pressEvent", &["string"] as &[&str]),
                    ("releaseEvent", &["string"] as &[&str]),
                    ("pressed", &["boolean"] as &[&str]),
                    ("disabled", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "counter-template",
                [
                    ("entity", &["object"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("onIncrement", &["function"] as &[&str]),
                    ("onDecrement", &["function"] as &[&str]),
                    ("onReset", &["function"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("showReset", &["boolean"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "crafting-recipe",
                [
                    ("inputs", &["array"] as &[&str]),
                    ("output", &["object"] as &[&str]),
                    ("canCraft", &["boolean"] as &[&str]),
                    ("onCraft", &["function"] as &[&str]),
                    ("craftEvent", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "cta-banner",
                [
                    ("title", &["string"] as &[&str]),
                    ("subtitle", &["string"] as &[&str]),
                    ("primaryAction", &["object"] as &[&str]),
                    ("secondaryAction", &["object"] as &[&str]),
                    ("background", &["string"] as &[&str]),
                    ("align", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "d-pad",
                [
                    ("onDirection", &["function"] as &[&str]),
                    ("directionEvent", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("includeDiagonals", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("disabled", &["boolean"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "damage-number",
                [
                    ("value", &["number"] as &[&str]),
                    ("type", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "dashboard-grid",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("columns", &["number"] as &[&str]),
                    ("gap", &["string"] as &[&str]),
                    ("cells", &["array"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "dashboard-layout",
                [
                    ("appName", &["string"] as &[&str]),
                    ("logo", &["node"] as &[&str]),
                    ("navItems", &["array"] as &[&str]),
                    ("user", &["object"] as &[&str]),
                    ("headerActions", &["node"] as &[&str]),
                    ("showSearch", &["boolean"] as &[&str]),
                    ("searchEvent", &["string"] as &[&str]),
                    ("onSearchSubmit", &["function"] as &[&str]),
                    ("topBarActions", &["array"] as &[&str]),
                    ("notifications", &["array"] as &[&str]),
                    ("notificationClickEvent", &["string"] as &[&str]),
                    ("onNotificationClick", &["function"] as &[&str]),
                    ("showThemeToggle", &["boolean"] as &[&str]),
                    ("sidebarFooter", &["node"] as &[&str]),
                    ("currentPath", &["string"] as &[&str]),
                    ("onSignOut", &["function"] as &[&str]),
                    ("layoutMode", &["string"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "data-grid",
                [
                    ("dragGroup", &["string"] as &[&str]),
                    ("accepts", &["string"] as &[&str]),
                    ("sortable", &["boolean"] as &[&str]),
                    ("dropEvent", &["string"] as &[&str]),
                    ("reorderEvent", &["string"] as &[&str]),
                    ("positionEvent", &["string"] as &[&str]),
                    ("dndItemIdField", &["string"] as &[&str]),
                    ("dndRoot", &["boolean"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("fields", &["array"] as &[&str]),
                    ("columns", &["array"] as &[&str]),
                    ("itemActions", &["array"] as &[&str]),
                    ("maxInlineActions", &["number"] as &[&str]),
                    ("scrollX", &["boolean"] as &[&str]),
                    ("cols", &["number"] as &[&str]),
                    ("gap", &["string"] as &[&str]),
                    ("minCardWidth", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("imageField", &["string"] as &[&str]),
                    ("selectable", &["boolean"] as &[&str]),
                    ("selectionEvent", &["string"] as &[&str]),
                    ("infiniteScroll", &["boolean"] as &[&str]),
                    ("loadMoreEvent", &["string"] as &[&str]),
                    ("hasMore", &["boolean"] as &[&str]),
                    ("children", &["function"] as &[&str]),
                    ("renderItem", &["function"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("look", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "data-list",
                [
                    ("dragGroup", &["string"] as &[&str]),
                    ("accepts", &["string"] as &[&str]),
                    ("sortable", &["boolean"] as &[&str]),
                    ("dropEvent", &["string"] as &[&str]),
                    ("reorderEvent", &["string"] as &[&str]),
                    ("positionEvent", &["string"] as &[&str]),
                    ("dndItemIdField", &["string"] as &[&str]),
                    ("dndRoot", &["boolean"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("fields", &["array"] as &[&str]),
                    ("columns", &["array"] as &[&str]),
                    ("itemActions", &["array"] as &[&str]),
                    ("maxInlineActions", &["number"] as &[&str]),
                    ("itemClickEvent", &["string"] as &[&str]),
                    ("gap", &["string"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("groupBy", &["string"] as &[&str]),
                    ("senderField", &["string"] as &[&str]),
                    ("currentUser", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("reorderable", &["boolean"] as &[&str]),
                    ("swipeLeftEvent", &["string"] as &[&str]),
                    ("swipeLeftActions", &["array"] as &[&str]),
                    ("swipeRightEvent", &["string"] as &[&str]),
                    ("swipeRightActions", &["array"] as &[&str]),
                    ("longPressEvent", &["string"] as &[&str]),
                    ("infiniteScroll", &["boolean"] as &[&str]),
                    ("loadMoreEvent", &["string"] as &[&str]),
                    ("hasMore", &["boolean"] as &[&str]),
                    ("children", &["function"] as &[&str]),
                    ("renderItem", &["function"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("look", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "date-range-picker",
                [
                    ("from", &["string"] as &[&str]),
                    ("to", &["string"] as &[&str]),
                    ("event", &["string"] as &[&str]),
                    ("onChange", &["function"] as &[&str]),
                    ("presets", &["array"] as &[&str]),
                    ("fromLabel", &["string"] as &[&str]),
                    ("toLabel", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "date-range-selector",
                [
                    ("options", &["array"] as &[&str]),
                    ("selected", &["string"] as &[&str]),
                    ("onSelect", &["function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "day-cell",
                [
                    ("date", &["date"] as &[&str]),
                    ("isToday", &["boolean"] as &[&str]),
                    ("onClick", &["function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "debugger-board",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("completeEvent", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "detail-panel",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("entity", &["object"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("subtitle", &["string"] as &[&str]),
                    ("status", &["object"] as &[&str]),
                    ("avatar", &["node"] as &[&str]),
                    ("sections", &["array"] as &[&str]),
                    ("actions", &["array"] as &[&str]),
                    ("footer", &["node"] as &[&str]),
                    ("slideOver", &["boolean"] as &[&str]),
                    ("fields", &["array"] as &[&str]),
                    ("fieldNames", &["array"] as &[&str]),
                    ("initialData", &["object"] as &[&str]),
                    ("mode", &["string"] as &[&str]),
                    ("position", &["string"] as &[&str]),
                    ("width", &["string"] as &[&str]),
                    ("displayFields", &["array"] as &[&str]),
                    ("showActions", &["boolean"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("dialog", [("className", &["string"] as &[&str]), ("children", &["node"] as &[&str])].into_iter().collect()),
            (
                "dialogue-box",
                [
                    ("dialogue", &["object"] as &[&str]),
                    ("typewriterSpeed", &["number"] as &[&str]),
                    ("position", &["string"] as &[&str]),
                    ("onComplete", &["function"] as &[&str]),
                    ("onChoice", &["function"] as &[&str]),
                    ("onAdvance", &["function"] as &[&str]),
                    ("completeEvent", &["string"] as &[&str]),
                    ("choiceEvent", &["string"] as &[&str]),
                    ("advanceEvent", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "dialogue-bubble",
                [
                    ("speaker", &["string"] as &[&str]),
                    ("text", &["string"] as &[&str]),
                    ("portrait", &["asset"] as &[&str]),
                    ("position", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "divider",
                [
                    ("orientation", &["string"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("doc-breadcrumb", [("items", &["array"] as &[&str]), ("className", &["string"] as &[&str])].into_iter().collect()),
            (
                "doc-pagination",
                [
                    ("prev", &["object"] as &[&str]),
                    ("next", &["object"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "doc-search",
                [
                    ("placeholder", &["string"] as &[&str]),
                    ("onSearch", &["function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("doc-sidebar", [("items", &["array"] as &[&str]), ("className", &["string"] as &[&str])].into_iter().collect()),
            (
                "doc-toc",
                [
                    ("items", &["array"] as &[&str]),
                    ("activeId", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "document-viewer",
                [
                    ("title", &["string"] as &[&str]),
                    ("src", &["asset"] as &[&str]),
                    ("content", &["string"] as &[&str]),
                    ("documentType", &["string"] as &[&str]),
                    ("currentPage", &["number"] as &[&str]),
                    ("totalPages", &["number"] as &[&str]),
                    ("height", &["number", "string"] as &[&str]),
                    ("showToolbar", &["boolean"] as &[&str]),
                    ("showDownload", &["boolean"] as &[&str]),
                    ("showPrint", &["boolean"] as &[&str]),
                    ("actions", &["array"] as &[&str]),
                    ("documents", &["array"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "drawer",
                [
                    ("isOpen", &["boolean"] as &[&str]),
                    ("onClose", &["function"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("footer", &["node"] as &[&str]),
                    ("position", &["string"] as &[&str]),
                    ("width", &["string"] as &[&str]),
                    ("showCloseButton", &["boolean"] as &[&str]),
                    ("closeOnOverlayClick", &["boolean"] as &[&str]),
                    ("closeOnEscape", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("closeEvent", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "drawer-slot",
                [
                    ("children", &["node"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("position", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("entity", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "edge-decoration",
                [
                    ("variant", &["string"] as &[&str]),
                    ("side", &["string"] as &[&str]),
                    ("opacity", &["number"] as &[&str]),
                    ("color", &["string"] as &[&str]),
                    ("strokeWidth", &["number"] as &[&str]),
                    ("width", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "empty-state",
                [
                    ("icon", &["icon", "string"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("message", &["string"] as &[&str]),
                    ("description", &["string"] as &[&str]),
                    ("actionLabel", &["string"] as &[&str]),
                    ("onAction", &["function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("destructive", &["boolean"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("actionEvent", &["string"] as &[&str]),
                    ("look", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "enemy-plate",
                [
                    ("name", &["string"] as &[&str]),
                    ("health", &["number"] as &[&str]),
                    ("maxHealth", &["number"] as &[&str]),
                    ("level", &["number"] as &[&str]),
                    ("effects", &["array"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "entity-cards",
                [
                    ("entity", &["object", "array"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("minCardWidth", &["number"] as &[&str]),
                    ("maxCols", &["number"] as &[&str]),
                    ("gap", &["string"] as &[&str]),
                    ("alignItems", &["string"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("fields", &["array"] as &[&str]),
                    ("fieldNames", &["array"] as &[&str]),
                    ("columns", &["array"] as &[&str]),
                    ("itemActions", &["array"] as &[&str]),
                    ("showTotal", &["boolean"] as &[&str]),
                    ("showAvatar", &["boolean"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("imageField", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "entity-list",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("entityType", &["string"] as &[&str]),
                    ("selectable", &["boolean"] as &[&str]),
                    ("itemActions", &["function", "array"] as &[&str]),
                    ("showDividers", &["boolean"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("emptyMessage", &["string"] as &[&str]),
                    ("renderItem", &["function"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("fields", &["array"] as &[&str]),
                    ("fieldNames", &["array"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "entity-table",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("fields", &["array"] as &[&str]),
                    ("columns", &["array"] as &[&str]),
                    ("itemActions", &["array"] as &[&str]),
                    ("emptyIcon", &["icon", "string"] as &[&str]),
                    ("emptyTitle", &["string"] as &[&str]),
                    ("emptyDescription", &["string"] as &[&str]),
                    ("emptyAction", &["object"] as &[&str]),
                    ("selectable", &["boolean"] as &[&str]),
                    ("searchable", &["boolean"] as &[&str]),
                    ("searchPlaceholder", &["string"] as &[&str]),
                    ("rowActions", &["array"] as &[&str]),
                    ("bulkActions", &["array"] as &[&str]),
                    ("headerActions", &["node"] as &[&str]),
                    ("showTotal", &["boolean"] as &[&str]),
                    ("look", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "error-boundary",
                [
                    ("children", &["node"] as &[&str]),
                    ("fallback", &["node", "function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("onError", &["function"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "error-state",
                [
                    ("title", &["string"] as &[&str]),
                    ("message", &["string"] as &[&str]),
                    ("description", &["string"] as &[&str]),
                    ("onRetry", &["function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("retryEvent", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "event-handler-board",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("stepDurationMs", &["number"] as &[&str]),
                    ("playEvent", &["string"] as &[&str]),
                    ("completeEvent", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "event-log",
                [
                    ("entries", &["array"] as &[&str]),
                    ("maxHeight", &["number"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "feature-card",
                [
                    ("icon", &["icon", "string"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("description", &["string"] as &[&str]),
                    ("href", &["string"] as &[&str]),
                    ("linkLabel", &["string"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("feature-detail-page-template", [("entity", &["object"] as &[&str]), ("className", &["string"] as &[&str])].into_iter().collect()),
            (
                "feature-grid",
                [
                    ("items", &["array"] as &[&str]),
                    ("columns", &["number"] as &[&str]),
                    ("gap", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "feature-grid-organism",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("columns", &["number"] as &[&str]),
                    ("heading", &["string"] as &[&str]),
                    ("subtitle", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "feature-renderer",
                [
                    ("features", &["array"] as &[&str]),
                    ("cellSize", &["number"] as &[&str]),
                    ("offsetX", &["number"] as &[&str]),
                    ("offsetZ", &["number"] as &[&str]),
                    ("onFeatureClick", &["function"] as &[&str]),
                    ("onFeatureHover", &["function"] as &[&str]),
                    ("selectedFeatureIds", &["array"] as &[&str]),
                    ("featureColors", &["object"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "file-tree",
                [
                    ("tree", &["array"] as &[&str]),
                    ("selectedPath", &["string"] as &[&str]),
                    ("onFileSelect", &["function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("indent", &["number"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "filter-group",
                [
                    ("entity", &["string"] as &[&str]),
                    ("filters", &["array"] as &[&str]),
                    ("onFilterChange", &["function"] as &[&str]),
                    ("onClearAll", &["function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("showIcon", &["boolean"] as &[&str]),
                    ("query", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("look", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "filter-pill",
                [
                    ("className", &["string"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("label", &["string", "number"] as &[&str]),
                    ("icon", &["icon", "string"] as &[&str]),
                    ("onRemove", &["function"] as &[&str]),
                    ("removable", &["boolean"] as &[&str]),
                    ("onClick", &["function"] as &[&str]),
                    ("clickEvent", &["string"] as &[&str]),
                    ("removeEvent", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "flex",
                [
                    ("direction", &["string"] as &[&str]),
                    ("wrap", &["string"] as &[&str]),
                    ("align", &["string"] as &[&str]),
                    ("justify", &["string"] as &[&str]),
                    ("gap", &["string"] as &[&str]),
                    ("inline", &["boolean"] as &[&str]),
                    ("grow", &["boolean", "number"] as &[&str]),
                    ("shrink", &["boolean", "number"] as &[&str]),
                    ("basis", &["string", "number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("as", &["component"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "flip-card",
                [
                    ("front", &["node"] as &[&str]),
                    ("back", &["node"] as &[&str]),
                    ("flipped", &["boolean"] as &[&str]),
                    ("onFlip", &["function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("height", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "flip-container",
                [
                    ("flipped", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("onClick", &["function"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "floating-action-button",
                [
                    ("action", &["string"] as &[&str]),
                    ("actionPayload", &["object"] as &[&str]),
                    ("actions", &["array"] as &[&str]),
                    ("icon", &["icon", "string"] as &[&str]),
                    ("onClick", &["function"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                    ("position", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "form",
                [
                    ("children", &["node"] as &[&str]),
                    ("onSubmit", &["string"] as &[&str]),
                    ("onCancel", &["string"] as &[&str]),
                    ("layout", &["string"] as &[&str]),
                    ("gap", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("entity", &["string", "object", "array"] as &[&str]),
                    ("mode", &["string"] as &[&str]),
                    ("fields", &["array"] as &[&str]),
                    ("initialData", &["object"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("submitLabel", &["string"] as &[&str]),
                    ("cancelLabel", &["string"] as &[&str]),
                    ("showCancel", &["boolean"] as &[&str]),
                    ("showSubmit", &["boolean"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("submitEvent", &["string"] as &[&str]),
                    ("cancelEvent", &["string"] as &[&str]),
                    ("relationsData", &["object"] as &[&str]),
                    ("relationsLoading", &["object"] as &[&str]),
                    ("conditionalFields", &["object", "boolean"] as &[&str]),
                    ("hiddenCalculations", &["array", "boolean"] as &[&str]),
                    ("violationTriggers", &["array", "boolean"] as &[&str]),
                    ("evaluationContext", &["object"] as &[&str]),
                    ("sections", &["array"] as &[&str]),
                    ("onFieldChange", &["function"] as &[&str]),
                    ("configPath", &["string"] as &[&str]),
                    ("repeatable", &["boolean"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "form-actions",
                [
                    ("children", &["node"] as &[&str]),
                    ("primary", &["object"] as &[&str]),
                    ("secondary", &["array"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("orientation", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("entity", &["string"] as &[&str]),
                    ("filters", &["array"] as &[&str]),
                    ("look", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "form-field",
                [
                    ("label", &["string"] as &[&str]),
                    ("required", &["boolean"] as &[&str]),
                    ("error", &["string"] as &[&str]),
                    ("hint", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "form-section",
                [
                    ("children", &["node"] as &[&str]),
                    ("onSubmit", &["string"] as &[&str]),
                    ("onCancel", &["string"] as &[&str]),
                    ("layout", &["string"] as &[&str]),
                    ("gap", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("entity", &["string", "object", "array"] as &[&str]),
                    ("mode", &["string"] as &[&str]),
                    ("fields", &["array"] as &[&str]),
                    ("initialData", &["object"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("submitLabel", &["string"] as &[&str]),
                    ("cancelLabel", &["string"] as &[&str]),
                    ("showCancel", &["boolean"] as &[&str]),
                    ("showSubmit", &["boolean"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("submitEvent", &["string"] as &[&str]),
                    ("cancelEvent", &["string"] as &[&str]),
                    ("relationsData", &["object"] as &[&str]),
                    ("relationsLoading", &["object"] as &[&str]),
                    ("conditionalFields", &["object", "boolean"] as &[&str]),
                    ("hiddenCalculations", &["array", "boolean"] as &[&str]),
                    ("violationTriggers", &["array", "boolean"] as &[&str]),
                    ("evaluationContext", &["object"] as &[&str]),
                    ("sections", &["array"] as &[&str]),
                    ("onFieldChange", &["function"] as &[&str]),
                    ("configPath", &["string"] as &[&str]),
                    ("repeatable", &["boolean"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "form-section-header",
                [
                    ("title", &["string"] as &[&str]),
                    ("subtitle", &["string"] as &[&str]),
                    ("isCollapsed", &["boolean"] as &[&str]),
                    ("onToggle", &["function"] as &[&str]),
                    ("badge", &["string"] as &[&str]),
                    ("badgeVariant", &["string"] as &[&str]),
                    ("icon", &["icon", "string"] as &[&str]),
                    ("hasErrors", &["boolean"] as &[&str]),
                    ("isComplete", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "game-audio-provider",
                [
                    ("manifest", &["object"] as &[&str]),
                    ("baseUrl", &["string"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("initialMuted", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("entity", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "game-audio-toggle",
                [
                    ("size", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("entity", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "game-canvas-2d",
                [
                    ("width", &["number"] as &[&str]),
                    ("height", &["number"] as &[&str]),
                    ("onDraw", &["function"] as &[&str]),
                    ("onTick", &["function"] as &[&str]),
                    ("tickEvent", &["string"] as &[&str]),
                    ("drawEvent", &["string"] as &[&str]),
                    ("fps", &["number"] as &[&str]),
                    ("backgroundImage", &["asset"] as &[&str]),
                    ("assetBaseUrl", &["asset"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "game-canvas-3d",
                [
                    ("className", &["string"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["string"] as &[&str]),
                    ("entity", &["string"] as &[&str]),
                    ("tiles", &["array"] as &[&str]),
                    ("units", &["array"] as &[&str]),
                    ("features", &["array"] as &[&str]),
                    ("events", &["array"] as &[&str]),
                    ("fogOfWar", &["array"] as &[&str]),
                    ("orientation", &["string"] as &[&str]),
                    ("cameraMode", &["string"] as &[&str]),
                    ("showGrid", &["boolean"] as &[&str]),
                    ("showCoordinates", &["boolean"] as &[&str]),
                    ("showTileInfo", &["boolean"] as &[&str]),
                    ("overlay", &["string"] as &[&str]),
                    ("shadows", &["boolean"] as &[&str]),
                    ("backgroundColor", &["string"] as &[&str]),
                    ("onTileClick", &["function"] as &[&str]),
                    ("onUnitClick", &["function"] as &[&str]),
                    ("onFeatureClick", &["function"] as &[&str]),
                    ("onCanvasClick", &["function"] as &[&str]),
                    ("onTileHover", &["function"] as &[&str]),
                    ("onUnitAnimation", &["function"] as &[&str]),
                    ("assetLoader", &["object"] as &[&str]),
                    ("tileRenderer", &["object"] as &[&str]),
                    ("unitRenderer", &["object"] as &[&str]),
                    ("featureRenderer", &["object"] as &[&str]),
                    ("preloadAssets", &["array"] as &[&str]),
                    ("tileClickEvent", &["string"] as &[&str]),
                    ("unitClickEvent", &["string"] as &[&str]),
                    ("featureClickEvent", &["string"] as &[&str]),
                    ("canvasClickEvent", &["string"] as &[&str]),
                    ("tileHoverEvent", &["string"] as &[&str]),
                    ("tileLeaveEvent", &["string"] as &[&str]),
                    ("unitAnimationEvent", &["string"] as &[&str]),
                    ("cameraChangeEvent", &["string"] as &[&str]),
                    ("loadingMessage", &["string"] as &[&str]),
                    ("useInstancing", &["boolean"] as &[&str]),
                    ("validMoves", &["array"] as &[&str]),
                    ("attackTargets", &["array"] as &[&str]),
                    ("selectedTileIds", &["array"] as &[&str]),
                    ("selectedUnitId", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "game-canvas-3d-battle-template",
                [
                    ("entity", &["object"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("cameraMode", &["string"] as &[&str]),
                    ("showGrid", &["boolean"] as &[&str]),
                    ("shadows", &["boolean"] as &[&str]),
                    ("backgroundColor", &["string"] as &[&str]),
                    ("tileClickEvent", &["string"] as &[&str]),
                    ("unitClickEvent", &["string"] as &[&str]),
                    ("unitAttackEvent", &["string"] as &[&str]),
                    ("unitMoveEvent", &["string"] as &[&str]),
                    ("endTurnEvent", &["string"] as &[&str]),
                    ("exitEvent", &["string"] as &[&str]),
                    ("selectedUnitId", &["string"] as &[&str]),
                    ("validMoves", &["array"] as &[&str]),
                    ("attackTargets", &["array"] as &[&str]),
                    ("showTurnIndicator", &["boolean"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "game-canvas-3d-castle-template",
                [
                    ("entity", &["object"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("cameraMode", &["string"] as &[&str]),
                    ("showGrid", &["boolean"] as &[&str]),
                    ("shadows", &["boolean"] as &[&str]),
                    ("backgroundColor", &["string"] as &[&str]),
                    ("buildingClickEvent", &["string"] as &[&str]),
                    ("unitClickEvent", &["string"] as &[&str]),
                    ("buildEvent", &["string"] as &[&str]),
                    ("recruitEvent", &["string"] as &[&str]),
                    ("exitEvent", &["string"] as &[&str]),
                    ("selectedBuildingId", &["string"] as &[&str]),
                    ("availableBuildSites", &["array"] as &[&str]),
                    ("showHeader", &["boolean"] as &[&str]),
                    ("selectedTileIds", &["array"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "game-canvas-3d-world-map-template",
                [
                    ("entity", &["object"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("cameraMode", &["string"] as &[&str]),
                    ("showGrid", &["boolean"] as &[&str]),
                    ("showCoordinates", &["boolean"] as &[&str]),
                    ("showTileInfo", &["boolean"] as &[&str]),
                    ("shadows", &["boolean"] as &[&str]),
                    ("backgroundColor", &["string"] as &[&str]),
                    ("tileClickEvent", &["string"] as &[&str]),
                    ("unitClickEvent", &["string"] as &[&str]),
                    ("featureClickEvent", &["string"] as &[&str]),
                    ("tileHoverEvent", &["string"] as &[&str]),
                    ("tileLeaveEvent", &["string"] as &[&str]),
                    ("cameraChangeEvent", &["string"] as &[&str]),
                    ("exitEvent", &["string"] as &[&str]),
                    ("selectedUnitId", &["string"] as &[&str]),
                    ("validMoves", &["array"] as &[&str]),
                    ("attackTargets", &["array"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "game-hud",
                [
                    ("position", &["string"] as &[&str]),
                    ("stats", &["array"] as &[&str]),
                    ("items", &["array"] as &[&str]),
                    ("elements", &["array"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("transparent", &["boolean"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "game-menu",
                [
                    ("title", &["string"] as &[&str]),
                    ("subtitle", &["string"] as &[&str]),
                    ("options", &["array"] as &[&str]),
                    ("menuItems", &["array"] as &[&str]),
                    ("onSelect", &["function"] as &[&str]),
                    ("eventBus", &["object"] as &[&str]),
                    ("background", &["string"] as &[&str]),
                    ("logo", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "game-over-screen",
                [
                    ("title", &["string"] as &[&str]),
                    ("message", &["string"] as &[&str]),
                    ("stats", &["array"] as &[&str]),
                    ("actions", &["array"] as &[&str]),
                    ("menuItems", &["array"] as &[&str]),
                    ("onAction", &["function"] as &[&str]),
                    ("eventBus", &["object"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("highScore", &["number", "string"] as &[&str]),
                    ("currentScore", &["number", "string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "game-shell",
                [
                    ("appName", &["string"] as &[&str]),
                    ("hud", &["node"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("showTopBar", &["boolean"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "game-template",
                [
                    ("entity", &["object"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("hud", &["node"] as &[&str]),
                    ("debugPanel", &["node"] as &[&str]),
                    ("showDebugPanel", &["boolean"] as &[&str]),
                    ("controls", &["object"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "generic-app-template",
                [
                    ("entity", &["object"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("subtitle", &["string"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("headerActions", &["node"] as &[&str]),
                    ("footer", &["node"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "geometric-pattern",
                [
                    ("variant", &["string"] as &[&str]),
                    ("mode", &["string"] as &[&str]),
                    ("opacity", &["number"] as &[&str]),
                    ("color", &["string"] as &[&str]),
                    ("scale", &["number"] as &[&str]),
                    ("strokeWidth", &["number"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("gradient-divider", [("color", &["string"] as &[&str]), ("className", &["string"] as &[&str])].into_iter().collect()),
            (
                "graph-canvas",
                [
                    ("title", &["string"] as &[&str]),
                    ("nodes", &["array"] as &[&str]),
                    ("edges", &["array"] as &[&str]),
                    ("height", &["number"] as &[&str]),
                    ("showLabels", &["boolean"] as &[&str]),
                    ("interactive", &["boolean"] as &[&str]),
                    ("draggable", &["boolean"] as &[&str]),
                    ("actions", &["array"] as &[&str]),
                    ("onNodeClick", &["function"] as &[&str]),
                    ("nodeClickEvent", &["string"] as &[&str]),
                    ("layout", &["string"] as &[&str]),
                    ("entity", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "graph-view",
                [
                    ("nodes", &["array"] as &[&str]),
                    ("edges", &["array"] as &[&str]),
                    ("onNodeClick", &["function"] as &[&str]),
                    ("onNodeHover", &["function"] as &[&str]),
                    ("width", &["number"] as &[&str]),
                    ("height", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("showLabels", &["boolean"] as &[&str]),
                    ("zoomToFit", &["boolean"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "grid",
                [
                    ("cols", &["number", "string", "object"] as &[&str]),
                    ("rows", &["number"] as &[&str]),
                    ("gap", &["string"] as &[&str]),
                    ("rowGap", &["string"] as &[&str]),
                    ("colGap", &["string"] as &[&str]),
                    ("alignItems", &["string"] as &[&str]),
                    ("justifyItems", &["string"] as &[&str]),
                    ("flow", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("style", &["object"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("as", &["component"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "header",
                [
                    ("logo", &["node"] as &[&str]),
                    ("logoSrc", &["asset"] as &[&str]),
                    ("brandName", &["string"] as &[&str]),
                    ("navigationItems", &["array"] as &[&str]),
                    ("showMenuToggle", &["boolean"] as &[&str]),
                    ("isMenuOpen", &["boolean"] as &[&str]),
                    ("onMenuToggle", &["function"] as &[&str]),
                    ("showSearch", &["boolean"] as &[&str]),
                    ("searchPlaceholder", &["string"] as &[&str]),
                    ("onSearch", &["function"] as &[&str]),
                    ("userAvatar", &["object"] as &[&str]),
                    ("userName", &["string"] as &[&str]),
                    ("onUserClick", &["function"] as &[&str]),
                    ("actions", &["node"] as &[&str]),
                    ("sticky", &["boolean"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("look", &["string"] as &[&str]),
                    ("onLogoClick", &["function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "health-bar",
                [
                    ("current", &["number"] as &[&str]),
                    ("max", &["number"] as &[&str]),
                    ("format", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("animated", &["boolean"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "health-panel",
                [
                    ("current", &["number"] as &[&str]),
                    ("max", &["number"] as &[&str]),
                    ("shield", &["number"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                    ("effects", &["array"] as &[&str]),
                    ("showNumbers", &["boolean"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "hero-organism",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("entity", &["object"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "hero-section",
                [
                    ("tag", &["string"] as &[&str]),
                    ("tagVariant", &["string"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("titleAccent", &["string"] as &[&str]),
                    ("subtitle", &["string"] as &[&str]),
                    ("primaryAction", &["object"] as &[&str]),
                    ("secondaryAction", &["object"] as &[&str]),
                    ("installCommand", &["string"] as &[&str]),
                    ("image", &["object"] as &[&str]),
                    ("imagePosition", &["string"] as &[&str]),
                    ("background", &["string"] as &[&str]),
                    ("align", &["string"] as &[&str]),
                    ("backgroundElement", &["node"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "icon",
                [
                    ("icon", &["icon", "string"] as &[&str]),
                    ("name", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("color", &["string"] as &[&str]),
                    ("animation", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("strokeWidth", &["number"] as &[&str]),
                    ("style", &["object"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "infinite-scroll-sentinel",
                [
                    ("loadMoreEvent", &["string"] as &[&str]),
                    ("loadMorePayload", &["object"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("hasMore", &["boolean"] as &[&str]),
                    ("threshold", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "input",
                [
                    ("className", &["string"] as &[&str]),
                    ("placeholder", &["string"] as &[&str]),
                    ("value", &["string", "number"] as &[&str]),
                    ("disabled", &["boolean"] as &[&str]),
                    ("action", &["string"] as &[&str]),
                    ("inputType", &["string"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                    ("helperText", &["string"] as &[&str]),
                    ("error", &["string"] as &[&str]),
                    ("leftIcon", &["icon", "string"] as &[&str]),
                    ("rightIcon", &["icon", "string"] as &[&str]),
                    ("icon", &["icon", "string"] as &[&str]),
                    ("clearable", &["boolean"] as &[&str]),
                    ("onClear", &["function", "string"] as &[&str]),
                    ("options", &["array"] as &[&str]),
                    ("rows", &["number"] as &[&str]),
                    ("onChange", &["function", "string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "input-group",
                [
                    ("leftAddon", &["node", "icon"] as &[&str]),
                    ("rightAddon", &["node", "icon"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "install-box",
                [
                    ("command", &["string"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "inventory-grid",
                [
                    ("items", &["array"] as &[&str]),
                    ("columns", &["number"] as &[&str]),
                    ("totalSlots", &["number"] as &[&str]),
                    ("selectedId", &["string"] as &[&str]),
                    ("onSelect", &["function"] as &[&str]),
                    ("selectEvent", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "inventory-panel",
                [
                    ("items", &["array"] as &[&str]),
                    ("slots", &["number"] as &[&str]),
                    ("columns", &["number"] as &[&str]),
                    ("selectedSlot", &["number"] as &[&str]),
                    ("onSelectSlot", &["function"] as &[&str]),
                    ("onUseItem", &["function"] as &[&str]),
                    ("onDropItem", &["function"] as &[&str]),
                    ("selectSlotEvent", &["string"] as &[&str]),
                    ("useItemEvent", &["string"] as &[&str]),
                    ("dropItemEvent", &["string"] as &[&str]),
                    ("showTooltips", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("slotSize", &["number"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "isometric-canvas",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("tiles", &["array"] as &[&str]),
                    ("units", &["array"] as &[&str]),
                    ("features", &["array"] as &[&str]),
                    ("selectedUnitId", &["string"] as &[&str]),
                    ("validMoves", &["array"] as &[&str]),
                    ("attackTargets", &["array"] as &[&str]),
                    ("hoveredTile", &["object"] as &[&str]),
                    ("onTileClick", &["function"] as &[&str]),
                    ("onUnitClick", &["function"] as &[&str]),
                    ("onTileHover", &["function"] as &[&str]),
                    ("onTileLeave", &["function"] as &[&str]),
                    ("tileClickEvent", &["string"] as &[&str]),
                    ("unitClickEvent", &["string"] as &[&str]),
                    ("tileHoverEvent", &["string"] as &[&str]),
                    ("tileLeaveEvent", &["string"] as &[&str]),
                    ("scale", &["number"] as &[&str]),
                    ("debug", &["boolean"] as &[&str]),
                    ("backgroundImage", &["asset"] as &[&str]),
                    ("showMinimap", &["boolean"] as &[&str]),
                    ("enableCamera", &["boolean"] as &[&str]),
                    ("unitScale", &["number"] as &[&str]),
                    ("boardWidth", &["number"] as &[&str]),
                    ("boardHeight", &["number"] as &[&str]),
                    ("diamondTopY", &["number"] as &[&str]),
                    ("getTerrainSprite", &["function"] as &[&str]),
                    ("getFeatureSprite", &["function"] as &[&str]),
                    ("getUnitSprite", &["function"] as &[&str]),
                    ("resolveUnitFrame", &["function"] as &[&str]),
                    ("effectSpriteUrls", &["array"] as &[&str]),
                    ("onDrawEffects", &["function"] as &[&str]),
                    ("hasActiveEffects", &["boolean"] as &[&str]),
                    ("assetBaseUrl", &["asset"] as &[&str]),
                    ("assetManifest", &["object"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "item-slot",
                [
                    ("icon", &["icon", "string"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                    ("quantity", &["number"] as &[&str]),
                    ("rarity", &["string"] as &[&str]),
                    ("empty", &["boolean"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("selected", &["boolean"] as &[&str]),
                    ("onClick", &["function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "jazari-state-machine",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("schema", &["object"] as &[&str]),
                    ("trait", &["object"] as &[&str]),
                    ("traitIndex", &["number"] as &[&str]),
                    ("entityFields", &["array"] as &[&str]),
                    ("direction", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "label",
                [
                    ("className", &["string"] as &[&str]),
                    ("text", &["string"] as &[&str]),
                    ("htmlFor", &["string"] as &[&str]),
                    ("required", &["boolean"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "landing-page-template",
                [
                    ("entity", &["object"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("featureColumns", &["number"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "law-reference-tooltip",
                [
                    ("reference", &["object"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("position", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "lightbox",
                [
                    ("images", &["array"] as &[&str]),
                    ("currentIndex", &["number"] as &[&str]),
                    ("isOpen", &["boolean"] as &[&str]),
                    ("showCounter", &["boolean"] as &[&str]),
                    ("closeAction", &["string"] as &[&str]),
                    ("onClose", &["function"] as &[&str]),
                    ("onIndexChange", &["function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "likert-scale",
                [
                    ("question", &["string"] as &[&str]),
                    ("options", &["array"] as &[&str]),
                    ("value", &["number", "string"] as &[&str]),
                    ("onChange", &["function"] as &[&str]),
                    ("changeEvent", &["string"] as &[&str]),
                    ("disabled", &["boolean"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "line-chart",
                [
                    ("data", &["array"] as &[&str]),
                    ("width", &["number"] as &[&str]),
                    ("height", &["number"] as &[&str]),
                    ("showGrid", &["boolean"] as &[&str]),
                    ("showValues", &["boolean"] as &[&str]),
                    ("showArea", &["boolean"] as &[&str]),
                    ("lineColor", &["string"] as &[&str]),
                    ("areaColor", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "loading-state",
                [
                    ("title", &["string"] as &[&str]),
                    ("message", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "map-view",
                [
                    ("markers", &["array"] as &[&str]),
                    ("routes", &["array"] as &[&str]),
                    ("centerLat", &["number"] as &[&str]),
                    ("centerLng", &["number"] as &[&str]),
                    ("zoom", &["number"] as &[&str]),
                    ("height", &["string"] as &[&str]),
                    ("onMarkerClick", &["function"] as &[&str]),
                    ("onMapClick", &["function"] as &[&str]),
                    ("mapClickEvent", &["string"] as &[&str]),
                    ("markerClickEvent", &["string"] as &[&str]),
                    ("showClickedPin", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("showAttribution", &["boolean"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "markdown-content",
                [
                    ("content", &["string"] as &[&str]),
                    ("direction", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "marketing-footer",
                [
                    ("columns", &["array"] as &[&str]),
                    ("copyright", &["string"] as &[&str]),
                    ("logo", &["object"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "master-detail",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("masterFields", &["array"] as &[&str]),
                    ("detailFields", &["array"] as &[&str]),
                    ("loading", &["boolean"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "master-detail-layout",
                [
                    ("master", &["node"] as &[&str]),
                    ("detail", &["node"] as &[&str]),
                    ("emptyDetail", &["node"] as &[&str]),
                    ("hasSelection", &["boolean"] as &[&str]),
                    ("masterWidth", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("masterClassName", &["string"] as &[&str]),
                    ("detailClassName", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "matrix-question",
                [
                    ("title", &["string"] as &[&str]),
                    ("rows", &["array"] as &[&str]),
                    ("columns", &["array"] as &[&str]),
                    ("values", &["object"] as &[&str]),
                    ("onChange", &["function"] as &[&str]),
                    ("changeEvent", &["string"] as &[&str]),
                    ("disabled", &["boolean"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "media-gallery",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("items", &["array"] as &[&str]),
                    ("columns", &["number"] as &[&str]),
                    ("selectable", &["boolean"] as &[&str]),
                    ("selectedItems", &["array"] as &[&str]),
                    ("selectionEvent", &["string"] as &[&str]),
                    ("showUpload", &["boolean"] as &[&str]),
                    ("actions", &["array"] as &[&str]),
                    ("aspectRatio", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "menu",
                [
                    ("trigger", &["node"] as &[&str]),
                    ("items", &["array"] as &[&str]),
                    ("position", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("header", &["node"] as &[&str]),
                    ("footer", &["node"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "meter",
                [
                    ("value", &["number"] as &[&str]),
                    ("min", &["number"] as &[&str]),
                    ("max", &["number"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                    ("unit", &["string"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("thresholds", &["array"] as &[&str]),
                    ("segments", &["number"] as &[&str]),
                    ("showValue", &["boolean"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("actions", &["array"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "mini-map",
                [
                    ("tiles", &["array"] as &[&str]),
                    ("units", &["array"] as &[&str]),
                    ("width", &["number"] as &[&str]),
                    ("height", &["number"] as &[&str]),
                    ("mapWidth", &["number"] as &[&str]),
                    ("mapHeight", &["number"] as &[&str]),
                    ("viewportRect", &["object"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "modal",
                [
                    ("isOpen", &["boolean"] as &[&str]),
                    ("onClose", &["function"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("footer", &["node"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("showCloseButton", &["boolean"] as &[&str]),
                    ("closeOnOverlayClick", &["boolean"] as &[&str]),
                    ("closeOnEscape", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("closeEvent", &["string"] as &[&str]),
                    ("swipeDownToClose", &["boolean"] as &[&str]),
                    ("look", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "modal-slot",
                [
                    ("children", &["node"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("entity", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("module-card", [("data", &["object"] as &[&str])].into_iter().collect()),
            (
                "navigation",
                [
                    ("items", &["array"] as &[&str]),
                    ("orientation", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "negotiator-board",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("completeEvent", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "notification",
                [
                    ("variant", &["string"] as &[&str]),
                    ("message", &["string"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("duration", &["number"] as &[&str]),
                    ("dismissible", &["boolean"] as &[&str]),
                    ("onDismiss", &["function"] as &[&str]),
                    ("actionLabel", &["string"] as &[&str]),
                    ("onAction", &["function"] as &[&str]),
                    ("badge", &["string", "number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("dismissEvent", &["string"] as &[&str]),
                    ("actionEvent", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "number-stepper",
                [
                    ("value", &["number"] as &[&str]),
                    ("min", &["number"] as &[&str]),
                    ("max", &["number"] as &[&str]),
                    ("step", &["number"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("disabled", &["boolean"] as &[&str]),
                    ("onChange", &["function"] as &[&str]),
                    ("action", &["string"] as &[&str]),
                    ("actionPayload", &["object"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "object-rule-panel",
                [
                    ("object", &["object"] as &[&str]),
                    ("onRulesChange", &["function"] as &[&str]),
                    ("disabled", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "option-constraint-group",
                [
                    ("groupId", &["string"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("description", &["string"] as &[&str]),
                    ("options", &["array"] as &[&str]),
                    ("constraint", &["object"] as &[&str]),
                    ("selected", &["array"] as &[&str]),
                    ("onChange", &["function"] as &[&str]),
                    ("changeEvent", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "orbital-visualization",
                [
                    ("schema", &["object"] as &[&str]),
                    ("complexity", &["number"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("showLabel", &["boolean"] as &[&str]),
                    ("animated", &["boolean"] as &[&str]),
                    ("onClick", &["function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("entity", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "overlay",
                [
                    ("isVisible", &["boolean"] as &[&str]),
                    ("onClick", &["function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("blur", &["boolean"] as &[&str]),
                    ("action", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "page-header",
                [
                    ("title", &["string", "number"] as &[&str]),
                    ("subtitle", &["string", "number"] as &[&str]),
                    ("showBack", &["boolean"] as &[&str]),
                    ("backEvent", &["string"] as &[&str]),
                    ("breadcrumbs", &["array"] as &[&str]),
                    ("status", &["object"] as &[&str]),
                    ("actions", &["array"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("tabs", &["array"] as &[&str]),
                    ("activeTab", &["string"] as &[&str]),
                    ("onTabChange", &["function"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "pagination",
                [
                    ("currentPage", &["number"] as &[&str]),
                    ("totalPages", &["number"] as &[&str]),
                    ("onPageChange", &["function"] as &[&str]),
                    ("showPageSize", &["boolean"] as &[&str]),
                    ("pageSizeOptions", &["array"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("onPageSizeChange", &["function"] as &[&str]),
                    ("showJumpToPage", &["boolean"] as &[&str]),
                    ("showTotal", &["boolean"] as &[&str]),
                    ("totalItems", &["number"] as &[&str]),
                    ("maxVisiblePages", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("pageChangeEvent", &["string"] as &[&str]),
                    ("pageSizeChangeEvent", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "pattern-tile",
                [
                    ("variant", &["string"] as &[&str]),
                    ("size", &["number"] as &[&str]),
                    ("color", &["string"] as &[&str]),
                    ("strokeWidth", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "platformer-canvas",
                [
                    ("player", &["object"] as &[&str]),
                    ("platforms", &["array"] as &[&str]),
                    ("worldWidth", &["number"] as &[&str]),
                    ("worldHeight", &["number"] as &[&str]),
                    ("canvasWidth", &["number"] as &[&str]),
                    ("canvasHeight", &["number"] as &[&str]),
                    ("followCamera", &["boolean"] as &[&str]),
                    ("bgColor", &["string"] as &[&str]),
                    ("playerSprite", &["asset"] as &[&str]),
                    ("tileSprites", &["object"] as &[&str]),
                    ("backgroundImage", &["asset"] as &[&str]),
                    ("assetBaseUrl", &["asset"] as &[&str]),
                    ("leftEvent", &["string"] as &[&str]),
                    ("rightEvent", &["string"] as &[&str]),
                    ("jumpEvent", &["string"] as &[&str]),
                    ("stopEvent", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "popover",
                [
                    ("content", &["node"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("position", &["string"] as &[&str]),
                    ("trigger", &["string"] as &[&str]),
                    ("showArrow", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "positioned-canvas",
                [
                    ("items", &["object", "array"] as &[&str]),
                    ("width", &["number"] as &[&str]),
                    ("height", &["number"] as &[&str]),
                    ("selectedId", &["string"] as &[&str]),
                    ("editable", &["boolean"] as &[&str]),
                    ("onSelect", &["function"] as &[&str]),
                    ("onMove", &["function"] as &[&str]),
                    ("selectEvent", &["string"] as &[&str]),
                    ("moveEvent", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "powerup-slots",
                [
                    ("active", &["array"] as &[&str]),
                    ("maxSlots", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "pricing-card",
                [
                    ("name", &["string"] as &[&str]),
                    ("price", &["string"] as &[&str]),
                    ("description", &["string"] as &[&str]),
                    ("features", &["array"] as &[&str]),
                    ("action", &["object"] as &[&str]),
                    ("highlighted", &["boolean"] as &[&str]),
                    ("badge", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("pricing-grid", [("plans", &["array"] as &[&str]), ("className", &["string"] as &[&str])].into_iter().collect()),
            (
                "pricing-organism",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("heading", &["string"] as &[&str]),
                    ("subtitle", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("pricing-page-template", [("entity", &["object"] as &[&str]), ("className", &["string"] as &[&str])].into_iter().collect()),
            (
                "progress-bar",
                [
                    ("value", &["number"] as &[&str]),
                    ("max", &["number"] as &[&str]),
                    ("progressType", &["string"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("color", &["string"] as &[&str]),
                    ("showPercentage", &["boolean"] as &[&str]),
                    ("showLabel", &["boolean"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("steps", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "progress-dots",
                [
                    ("count", &["number"] as &[&str]),
                    ("currentIndex", &["number"] as &[&str]),
                    ("getState", &["function"] as &[&str]),
                    ("onDotClick", &["function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("pull-quote", [("children", &["string"] as &[&str]), ("className", &["string"] as &[&str])].into_iter().collect()),
            (
                "pull-to-refresh",
                [
                    ("refreshEvent", &["string"] as &[&str]),
                    ("refreshPayload", &["object"] as &[&str]),
                    ("threshold", &["number"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "qr-scanner",
                [
                    ("onScan", &["function"] as &[&str]),
                    ("scanEvent", &["string"] as &[&str]),
                    ("onError", &["function"] as &[&str]),
                    ("facingMode", &["string"] as &[&str]),
                    ("paused", &["boolean"] as &[&str]),
                    ("showOverlay", &["boolean"] as &[&str]),
                    ("showCameraControls", &["boolean"] as &[&str]),
                    ("fallback", &["node"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "quest-tracker",
                [
                    ("quests", &["array"] as &[&str]),
                    ("activeQuestId", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "quiz-block",
                [
                    ("question", &["string"] as &[&str]),
                    ("answer", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "radio",
                [
                    ("className", &["string"] as &[&str]),
                    ("options", &["array"] as &[&str]),
                    ("value", &["string"] as &[&str]),
                    ("action", &["string"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                    ("helperText", &["string"] as &[&str]),
                    ("error", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "range-slider",
                [
                    ("className", &["string"] as &[&str]),
                    ("min", &["number"] as &[&str]),
                    ("max", &["number"] as &[&str]),
                    ("value", &["number"] as &[&str]),
                    ("step", &["number"] as &[&str]),
                    ("showTooltip", &["boolean"] as &[&str]),
                    ("showTicks", &["boolean"] as &[&str]),
                    ("buffered", &["number"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("disabled", &["boolean"] as &[&str]),
                    ("action", &["string"] as &[&str]),
                    ("actionPayload", &["object"] as &[&str]),
                    ("onChange", &["function"] as &[&str]),
                    ("formatValue", &["function"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "relation-select",
                [
                    ("value", &["string"] as &[&str]),
                    ("onChange", &["function"] as &[&str]),
                    ("options", &["array"] as &[&str]),
                    ("placeholder", &["string"] as &[&str]),
                    ("required", &["boolean"] as &[&str]),
                    ("disabled", &["boolean"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["string"] as &[&str]),
                    ("clearable", &["boolean"] as &[&str]),
                    ("name", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("searchPlaceholder", &["string"] as &[&str]),
                    ("emptyMessage", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "repeatable-form-section",
                [
                    ("sectionType", &["string"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("items", &["array"] as &[&str]),
                    ("renderItem", &["function"] as &[&str]),
                    ("minItems", &["number"] as &[&str]),
                    ("maxItems", &["number"] as &[&str]),
                    ("allowReorder", &["boolean"] as &[&str]),
                    ("addLabel", &["string"] as &[&str]),
                    ("emptyMessage", &["string"] as &[&str]),
                    ("readOnly", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("onAdd", &["function"] as &[&str]),
                    ("onRemove", &["function"] as &[&str]),
                    ("onReorder", &["function"] as &[&str]),
                    ("trackAddedInState", &["boolean"] as &[&str]),
                    ("currentState", &["string"] as &[&str]),
                    ("showAuditInfo", &["boolean"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "reply-tree",
                [
                    ("nodes", &["object", "array"] as &[&str]),
                    ("maxDepth", &["number"] as &[&str]),
                    ("onVote", &["function"] as &[&str]),
                    ("onReply", &["function"] as &[&str]),
                    ("onFlag", &["function"] as &[&str]),
                    ("onContinueThread", &["function"] as &[&str]),
                    ("voteEvent", &["string"] as &[&str]),
                    ("replyEvent", &["string"] as &[&str]),
                    ("flagEvent", &["string"] as &[&str]),
                    ("continueThreadEvent", &["string"] as &[&str]),
                    ("showActions", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "resource-bar",
                [
                    ("resources", &["array"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "resource-counter",
                [
                    ("icon", &["icon", "string"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                    ("value", &["number"] as &[&str]),
                    ("max", &["number"] as &[&str]),
                    ("color", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "rich-block-editor",
                [
                    ("initialBlocks", &["object", "array"] as &[&str]),
                    ("onChange", &["function"] as &[&str]),
                    ("changeEvent", &["string"] as &[&str]),
                    ("readOnly", &["boolean"] as &[&str]),
                    ("placeholder", &["string"] as &[&str]),
                    ("enableBlocks", &["boolean"] as &[&str]),
                    ("showToolbar", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "rule-editor",
                [
                    ("rule", &["object"] as &[&str]),
                    ("availableEvents", &["array"] as &[&str]),
                    ("availableActions", &["array"] as &[&str]),
                    ("onChange", &["function"] as &[&str]),
                    ("onRemove", &["function"] as &[&str]),
                    ("disabled", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "runtime-debugger",
                [
                    ("position", &["string"] as &[&str]),
                    ("defaultCollapsed", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("mode", &["string"] as &[&str]),
                    ("defaultTab", &["string"] as &[&str]),
                    ("schema", &["object"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("scaled-diagram", [("children", &["node"] as &[&str]), ("className", &["string"] as &[&str])].into_iter().collect()),
            (
                "score-board",
                [
                    ("score", &["number"] as &[&str]),
                    ("highScore", &["number"] as &[&str]),
                    ("combo", &["number"] as &[&str]),
                    ("multiplier", &["number"] as &[&str]),
                    ("level", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "score-display",
                [
                    ("value", &["number"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                    ("icon", &["icon", "string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("animated", &["boolean"] as &[&str]),
                    ("locale", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "search-input",
                [
                    ("value", &["string"] as &[&str]),
                    ("onSearch", &["function"] as &[&str]),
                    ("debounceMs", &["number"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("placeholder", &["string"] as &[&str]),
                    ("clearable", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("event", &["string"] as &[&str]),
                    ("entity", &["string"] as &[&str]),
                    ("query", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "section",
                [
                    ("title", &["string"] as &[&str]),
                    ("description", &["string"] as &[&str]),
                    ("action", &["node"] as &[&str]),
                    ("padding", &["string"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("divider", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("headerClassName", &["string"] as &[&str]),
                    ("contentClassName", &["string"] as &[&str]),
                    ("as", &["component"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("entity", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "section-header",
                [
                    ("title", &["string"] as &[&str]),
                    ("subtitle", &["string"] as &[&str]),
                    ("align", &["string"] as &[&str]),
                    ("level", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "select",
                [
                    ("className", &["string"] as &[&str]),
                    ("options", &["array"] as &[&str]),
                    ("groups", &["array"] as &[&str]),
                    ("placeholder", &["string"] as &[&str]),
                    ("value", &["string", "array"] as &[&str]),
                    ("action", &["string"] as &[&str]),
                    ("error", &["string"] as &[&str]),
                    ("multiple", &["boolean"] as &[&str]),
                    ("searchable", &["boolean"] as &[&str]),
                    ("clearable", &["boolean"] as &[&str]),
                    ("onChange", &["function", "string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "sequence-bar",
                [
                    ("slots", &["array"] as &[&str]),
                    ("maxSlots", &["number"] as &[&str]),
                    ("onSlotDrop", &["function"] as &[&str]),
                    ("onSlotRemove", &["function"] as &[&str]),
                    ("playing", &["boolean"] as &[&str]),
                    ("currentStep", &["number"] as &[&str]),
                    ("categoryColors", &["object"] as &[&str]),
                    ("slotFeedback", &["array"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "sequencer-board",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("categoryColors", &["object"] as &[&str]),
                    ("stepDurationMs", &["number"] as &[&str]),
                    ("playEvent", &["string"] as &[&str]),
                    ("completeEvent", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("service-catalog", [("services", &["array"] as &[&str]), ("className", &["string"] as &[&str])].into_iter().collect()),
            (
                "showcase-card",
                [
                    ("title", &["string"] as &[&str]),
                    ("description", &["string"] as &[&str]),
                    ("image", &["object"] as &[&str]),
                    ("href", &["string"] as &[&str]),
                    ("badge", &["string"] as &[&str]),
                    ("accentColor", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "showcase-organism",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("columns", &["number"] as &[&str]),
                    ("heading", &["string"] as &[&str]),
                    ("subtitle", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "side-panel",
                [
                    ("title", &["string"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("isOpen", &["boolean"] as &[&str]),
                    ("onClose", &["function"] as &[&str]),
                    ("width", &["string"] as &[&str]),
                    ("position", &["string"] as &[&str]),
                    ("showOverlay", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("closeEvent", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "sidebar",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("logo", &["node"] as &[&str]),
                    ("logoSrc", &["asset"] as &[&str]),
                    ("brandName", &["string"] as &[&str]),
                    ("items", &["array"] as &[&str]),
                    ("userSection", &["node"] as &[&str]),
                    ("footerContent", &["node"] as &[&str]),
                    ("collapsed", &["boolean"] as &[&str]),
                    ("defaultCollapsed", &["boolean"] as &[&str]),
                    ("collapseChangeEvent", &["string"] as &[&str]),
                    ("hideCollapseButton", &["boolean"] as &[&str]),
                    ("showCloseButton", &["boolean"] as &[&str]),
                    ("closeEvent", &["string"] as &[&str]),
                    ("logoClickEvent", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "signature-pad",
                [
                    ("label", &["string"] as &[&str]),
                    ("helperText", &["string"] as &[&str]),
                    ("strokeColor", &["string"] as &[&str]),
                    ("strokeWidth", &["number"] as &[&str]),
                    ("height", &["number"] as &[&str]),
                    ("readOnly", &["boolean"] as &[&str]),
                    ("value", &["string"] as &[&str]),
                    ("onChange", &["function"] as &[&str]),
                    ("signEvent", &["string"] as &[&str]),
                    ("clearEvent", &["string"] as &[&str]),
                    ("entity", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "simple-grid",
                [
                    ("minChildWidth", &["number", "string"] as &[&str]),
                    ("maxCols", &["number"] as &[&str]),
                    ("cols", &["number"] as &[&str]),
                    ("gap", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "simulation-canvas",
                [
                    ("preset", &["string", "object"] as &[&str]),
                    ("width", &["number"] as &[&str]),
                    ("height", &["number"] as &[&str]),
                    ("running", &["boolean"] as &[&str]),
                    ("speed", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "simulation-controls",
                [
                    ("running", &["boolean"] as &[&str]),
                    ("speed", &["number"] as &[&str]),
                    ("parameters", &["object"] as &[&str]),
                    ("onPlay", &["function"] as &[&str]),
                    ("onPause", &["function"] as &[&str]),
                    ("onStep", &["function"] as &[&str]),
                    ("onReset", &["function"] as &[&str]),
                    ("onSpeedChange", &["function"] as &[&str]),
                    ("onParameterChange", &["function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "simulation-graph",
                [
                    ("label", &["string"] as &[&str]),
                    ("unit", &["string"] as &[&str]),
                    ("data", &["array"] as &[&str]),
                    ("maxPoints", &["number"] as &[&str]),
                    ("width", &["number"] as &[&str]),
                    ("height", &["number"] as &[&str]),
                    ("color", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "simulator-board",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("completeEvent", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "skeleton",
                [
                    ("variant", &["string"] as &[&str]),
                    ("rows", &["number"] as &[&str]),
                    ("columns", &["number"] as &[&str]),
                    ("fields", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "social-proof",
                [
                    ("items", &["array"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "sortable-list",
                [
                    ("items", &["object", "array"] as &[&str]),
                    ("renderItem", &["function"] as &[&str]),
                    ("reorderEvent", &["string"] as &[&str]),
                    ("reorderPayload", &["object"] as &[&str]),
                    ("dragHandlePosition", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "spacer",
                [
                    ("size", &["string"] as &[&str]),
                    ("axis", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "sparkline",
                [
                    ("data", &["array"] as &[&str]),
                    ("color", &["string"] as &[&str]),
                    ("width", &["number"] as &[&str]),
                    ("height", &["number"] as &[&str]),
                    ("strokeWidth", &["number"] as &[&str]),
                    ("fill", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "spinner",
                [
                    ("className", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("overlay", &["boolean"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "split",
                [
                    ("ratio", &["string"] as &[&str]),
                    ("gap", &["string"] as &[&str]),
                    ("reverse", &["boolean"] as &[&str]),
                    ("stackOnMobile", &["boolean"] as &[&str]),
                    ("stackBreakpoint", &["string"] as &[&str]),
                    ("align", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("leftClassName", &["string"] as &[&str]),
                    ("rightClassName", &["string"] as &[&str]),
                    ("children", &["array"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("entity", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "split-pane",
                [
                    ("direction", &["string"] as &[&str]),
                    ("ratio", &["number"] as &[&str]),
                    ("minSize", &["number"] as &[&str]),
                    ("resizable", &["boolean"] as &[&str]),
                    ("left", &["node"] as &[&str]),
                    ("right", &["node"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("leftClassName", &["string"] as &[&str]),
                    ("rightClassName", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "split-section",
                [
                    ("title", &["string"] as &[&str]),
                    ("description", &["string", "node"] as &[&str]),
                    ("bullets", &["array"] as &[&str]),
                    ("image", &["object"] as &[&str]),
                    ("imagePosition", &["string"] as &[&str]),
                    ("background", &["string"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "sprite",
                [
                    ("spritesheet", &["asset"] as &[&str]),
                    ("frameWidth", &["number"] as &[&str]),
                    ("frameHeight", &["number"] as &[&str]),
                    ("frame", &["number"] as &[&str]),
                    ("x", &["number"] as &[&str]),
                    ("y", &["number"] as &[&str]),
                    ("scale", &["number"] as &[&str]),
                    ("flipX", &["boolean"] as &[&str]),
                    ("flipY", &["boolean"] as &[&str]),
                    ("rotation", &["number"] as &[&str]),
                    ("opacity", &["number"] as &[&str]),
                    ("zIndex", &["number"] as &[&str]),
                    ("columns", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("onClick", &["function"] as &[&str]),
                    ("action", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "stack",
                [
                    ("direction", &["string"] as &[&str]),
                    ("gap", &["string"] as &[&str]),
                    ("align", &["string"] as &[&str]),
                    ("justify", &["string"] as &[&str]),
                    ("wrap", &["boolean"] as &[&str]),
                    ("reverse", &["boolean"] as &[&str]),
                    ("flex", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("style", &["object"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("as", &["component"] as &[&str]),
                    ("onClick", &["function"] as &[&str]),
                    ("onKeyDown", &["function"] as &[&str]),
                    ("role", &["string"] as &[&str]),
                    ("tabIndex", &["number"] as &[&str]),
                    ("action", &["string"] as &[&str]),
                    ("actionPayload", &["object"] as &[&str]),
                    ("responsive", &["boolean"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "star-rating",
                [
                    ("value", &["number"] as &[&str]),
                    ("max", &["number"] as &[&str]),
                    ("readOnly", &["boolean"] as &[&str]),
                    ("precision", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("action", &["string"] as &[&str]),
                    ("actionPayload", &["object"] as &[&str]),
                    ("onChange", &["function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "stat-badge",
                [
                    ("label", &["string"] as &[&str]),
                    ("value", &["number", "string"] as &[&str]),
                    ("max", &["number"] as &[&str]),
                    ("source", &["string"] as &[&str]),
                    ("field", &["string"] as &[&str]),
                    ("format", &["string"] as &[&str]),
                    ("icon", &["icon", "string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "stat-card",
                [
                    ("value", &["string"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "stat-display",
                [
                    ("label", &["string"] as &[&str]),
                    ("value", &["number", "string"] as &[&str]),
                    ("max", &["number"] as &[&str]),
                    ("target", &["number"] as &[&str]),
                    ("trend", &["number"] as &[&str]),
                    ("trendPolarity", &["string"] as &[&str]),
                    ("trendFormat", &["string"] as &[&str]),
                    ("sparklineData", &["array"] as &[&str]),
                    ("clickEvent", &["string"] as &[&str]),
                    ("prefix", &["string"] as &[&str]),
                    ("suffix", &["string"] as &[&str]),
                    ("icon", &["icon", "string"] as &[&str]),
                    ("iconBg", &["string"] as &[&str]),
                    ("iconColor", &["string"] as &[&str]),
                    ("format", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("compact", &["boolean"] as &[&str]),
                    ("look", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "state-architect-board",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("stepDurationMs", &["number"] as &[&str]),
                    ("testEvent", &["string"] as &[&str]),
                    ("completeEvent", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "state-indicator",
                [
                    ("state", &["string"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("animated", &["boolean"] as &[&str]),
                    ("stateStyles", &["object"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "state-machine-view",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("layoutData", &["object"] as &[&str]),
                    ("renderStateNode", &["function"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "state-node",
                [
                    ("name", &["string"] as &[&str]),
                    ("isCurrent", &["boolean"] as &[&str]),
                    ("isSelected", &["boolean"] as &[&str]),
                    ("isInitial", &["boolean"] as &[&str]),
                    ("position", &["object"] as &[&str]),
                    ("onClick", &["function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "stats-grid",
                [
                    ("stats", &["array"] as &[&str]),
                    ("columns", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "stats-organism",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("columns", &["number"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "status-dot",
                [
                    ("className", &["string"] as &[&str]),
                    ("status", &["string"] as &[&str]),
                    ("pulse", &["boolean"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "status-effect",
                [
                    ("icon", &["icon", "string"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                    ("duration", &["number"] as &[&str]),
                    ("stacks", &["number"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "step-flow",
                [
                    ("steps", &["array"] as &[&str]),
                    ("orientation", &["string"] as &[&str]),
                    ("showConnectors", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "step-flow-organism",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("orientation", &["string"] as &[&str]),
                    ("showConnectors", &["boolean"] as &[&str]),
                    ("heading", &["string"] as &[&str]),
                    ("subtitle", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "svg-branch",
                [
                    ("x", &["number"] as &[&str]),
                    ("y", &["number"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("branches", &["number"] as &[&str]),
                    ("size", &["number"] as &[&str]),
                    ("color", &["string"] as &[&str]),
                    ("opacity", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("asRoot", &["boolean"] as &[&str]),
                    ("width", &["number"] as &[&str]),
                    ("height", &["number"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "svg-connection",
                [
                    ("x1", &["number"] as &[&str]),
                    ("y1", &["number"] as &[&str]),
                    ("x2", &["number"] as &[&str]),
                    ("y2", &["number"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("color", &["string"] as &[&str]),
                    ("strokeWidth", &["number"] as &[&str]),
                    ("opacity", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("asRoot", &["boolean"] as &[&str]),
                    ("width", &["number"] as &[&str]),
                    ("height", &["number"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "svg-flow",
                [
                    ("points", &["array"] as &[&str]),
                    ("color", &["string"] as &[&str]),
                    ("strokeWidth", &["number"] as &[&str]),
                    ("animated", &["boolean"] as &[&str]),
                    ("opacity", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("asRoot", &["boolean"] as &[&str]),
                    ("width", &["number"] as &[&str]),
                    ("height", &["number"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "svg-grid",
                [
                    ("x", &["number"] as &[&str]),
                    ("y", &["number"] as &[&str]),
                    ("cols", &["number"] as &[&str]),
                    ("rows", &["number"] as &[&str]),
                    ("spacing", &["number"] as &[&str]),
                    ("nodeRadius", &["number"] as &[&str]),
                    ("color", &["string"] as &[&str]),
                    ("opacity", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("highlights", &["array"] as &[&str]),
                    ("asRoot", &["boolean"] as &[&str]),
                    ("width", &["number"] as &[&str]),
                    ("height", &["number"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "svg-lobe",
                [
                    ("cx", &["number"] as &[&str]),
                    ("cy", &["number"] as &[&str]),
                    ("rx", &["number"] as &[&str]),
                    ("ry", &["number"] as &[&str]),
                    ("rotation", &["number"] as &[&str]),
                    ("shells", &["number"] as &[&str]),
                    ("color", &["string"] as &[&str]),
                    ("opacity", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("asRoot", &["boolean"] as &[&str]),
                    ("width", &["number"] as &[&str]),
                    ("height", &["number"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "svg-mesh",
                [
                    ("cx", &["number"] as &[&str]),
                    ("cy", &["number"] as &[&str]),
                    ("nodes", &["number"] as &[&str]),
                    ("radius", &["number"] as &[&str]),
                    ("color", &["string"] as &[&str]),
                    ("connectionDensity", &["number"] as &[&str]),
                    ("opacity", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("asRoot", &["boolean"] as &[&str]),
                    ("width", &["number"] as &[&str]),
                    ("height", &["number"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "svg-morph",
                [
                    ("x", &["number"] as &[&str]),
                    ("y", &["number"] as &[&str]),
                    ("size", &["number"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("color", &["string"] as &[&str]),
                    ("opacity", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("asRoot", &["boolean"] as &[&str]),
                    ("width", &["number"] as &[&str]),
                    ("height", &["number"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "svg-node",
                [
                    ("x", &["number"] as &[&str]),
                    ("y", &["number"] as &[&str]),
                    ("r", &["number"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("color", &["string"] as &[&str]),
                    ("opacity", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                    ("asRoot", &["boolean"] as &[&str]),
                    ("width", &["number"] as &[&str]),
                    ("height", &["number"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "svg-pulse",
                [
                    ("cx", &["number"] as &[&str]),
                    ("cy", &["number"] as &[&str]),
                    ("rings", &["number"] as &[&str]),
                    ("maxRadius", &["number"] as &[&str]),
                    ("color", &["string"] as &[&str]),
                    ("animated", &["boolean"] as &[&str]),
                    ("opacity", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("asRoot", &["boolean"] as &[&str]),
                    ("width", &["number"] as &[&str]),
                    ("height", &["number"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "svg-ring",
                [
                    ("cx", &["number"] as &[&str]),
                    ("cy", &["number"] as &[&str]),
                    ("r", &["number"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("color", &["string"] as &[&str]),
                    ("strokeWidth", &["number"] as &[&str]),
                    ("opacity", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                    ("asRoot", &["boolean"] as &[&str]),
                    ("width", &["number"] as &[&str]),
                    ("height", &["number"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "svg-shield",
                [
                    ("x", &["number"] as &[&str]),
                    ("y", &["number"] as &[&str]),
                    ("size", &["number"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("color", &["string"] as &[&str]),
                    ("opacity", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("asRoot", &["boolean"] as &[&str]),
                    ("width", &["number"] as &[&str]),
                    ("height", &["number"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "svg-stack",
                [
                    ("x", &["number"] as &[&str]),
                    ("y", &["number"] as &[&str]),
                    ("layers", &["number"] as &[&str]),
                    ("width", &["number"] as &[&str]),
                    ("height", &["number"] as &[&str]),
                    ("color", &["string"] as &[&str]),
                    ("opacity", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("labels", &["array"] as &[&str]),
                    ("asRoot", &["boolean"] as &[&str]),
                    ("svgWidth", &["number"] as &[&str]),
                    ("svgHeight", &["number"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "swipeable-row",
                [
                    ("leftActions", &["array"] as &[&str]),
                    ("rightActions", &["array"] as &[&str]),
                    ("threshold", &["number"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("itemData", &["object"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "switch",
                [
                    ("checked", &["boolean"] as &[&str]),
                    ("defaultChecked", &["boolean"] as &[&str]),
                    ("onChange", &["function"] as &[&str]),
                    ("disabled", &["boolean"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                    ("id", &["string"] as &[&str]),
                    ("name", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "tabbed-container",
                [
                    ("tabs", &["array"] as &[&str]),
                    ("defaultTab", &["string"] as &[&str]),
                    ("activeTab", &["string"] as &[&str]),
                    ("onTabChange", &["function"] as &[&str]),
                    ("position", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "table-view",
                [
                    ("dragGroup", &["string"] as &[&str]),
                    ("accepts", &["string"] as &[&str]),
                    ("sortable", &["boolean"] as &[&str]),
                    ("dropEvent", &["string"] as &[&str]),
                    ("reorderEvent", &["string"] as &[&str]),
                    ("positionEvent", &["string"] as &[&str]),
                    ("dndItemIdField", &["string"] as &[&str]),
                    ("dndRoot", &["boolean"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("columns", &["array"] as &[&str]),
                    ("fields", &["array"] as &[&str]),
                    ("itemActions", &["array"] as &[&str]),
                    ("maxInlineActions", &["number"] as &[&str]),
                    ("selectable", &["boolean"] as &[&str]),
                    ("selectEvent", &["string"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("sortEvent", &["string"] as &[&str]),
                    ("sortColumn", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("emptyMessage", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("groupBy", &["string"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("children", &["function"] as &[&str]),
                    ("renderItem", &["function"] as &[&str]),
                    ("look", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "tabs",
                [
                    ("items", &["array"] as &[&str]),
                    ("tabs", &["array"] as &[&str]),
                    ("defaultActiveTab", &["string"] as &[&str]),
                    ("activeTab", &["string"] as &[&str]),
                    ("onTabChange", &["function"] as &[&str]),
                    ("tabChangeEvent", &["string"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("orientation", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "tag-cloud",
                [
                    ("tags", &["array"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "tag-input",
                [
                    ("value", &["array"] as &[&str]),
                    ("onChange", &["function"] as &[&str]),
                    ("placeholder", &["string"] as &[&str]),
                    ("disabled", &["boolean"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("unique", &["boolean"] as &[&str]),
                    ("helperText", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("addEvent", &["string"] as &[&str]),
                    ("removeEvent", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "team-card",
                [
                    ("name", &["string"] as &[&str]),
                    ("nameAr", &["string"] as &[&str]),
                    ("role", &["string"] as &[&str]),
                    ("bio", &["string"] as &[&str]),
                    ("avatar", &["asset", "object"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "team-organism",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("sortBy", &["string"] as &[&str]),
                    ("sortDirection", &["string"] as &[&str]),
                    ("searchValue", &["string"] as &[&str]),
                    ("page", &["number"] as &[&str]),
                    ("pageSize", &["number"] as &[&str]),
                    ("totalCount", &["number"] as &[&str]),
                    ("activeFilters", &["object"] as &[&str]),
                    ("selectedIds", &["array"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("heading", &["string"] as &[&str]),
                    ("subtitle", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "text-highlight",
                [
                    ("highlightType", &["string"] as &[&str]),
                    ("isActive", &["boolean"] as &[&str]),
                    ("onClick", &["function"] as &[&str]),
                    ("onMouseEnter", &["function"] as &[&str]),
                    ("onMouseLeave", &["function"] as &[&str]),
                    ("annotationId", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("action", &["string"] as &[&str]),
                    ("hoverEvent", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "textarea",
                [
                    ("className", &["string"] as &[&str]),
                    ("placeholder", &["string"] as &[&str]),
                    ("rows", &["number"] as &[&str]),
                    ("action", &["string"] as &[&str]),
                    ("error", &["string"] as &[&str]),
                    ("onChange", &["function", "string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "theme-toggle",
                [
                    ("className", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("showLabel", &["boolean"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "time-slot-cell",
                [
                    ("time", &["string"] as &[&str]),
                    ("onClick", &["function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("isOccupied", &["boolean"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "timeline",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("items", &["array"] as &[&str]),
                    ("fields", &["array"] as &[&str]),
                    ("itemActions", &["array"] as &[&str]),
                    ("look", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "timer-display",
                [
                    ("seconds", &["number"] as &[&str]),
                    ("running", &["boolean"] as &[&str]),
                    ("format", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("lowThreshold", &["number"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "toast-slot",
                [
                    ("children", &["node"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("title", &["string"] as &[&str]),
                    ("duration", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("entity", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "tooltip",
                [
                    ("content", &["node"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                    ("position", &["string"] as &[&str]),
                    ("delay", &["number"] as &[&str]),
                    ("hideDelay", &["number"] as &[&str]),
                    ("showArrow", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("trait-frame", [("traitName", &["string"] as &[&str]), ("fallback", &["node"] as &[&str])].into_iter().collect()),
            (
                "trait-slot",
                [
                    ("slotNumber", &["number"] as &[&str]),
                    ("equippedItem", &["object"] as &[&str]),
                    ("locked", &["boolean"] as &[&str]),
                    ("lockLabel", &["string"] as &[&str]),
                    ("selected", &["boolean"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("showTooltip", &["boolean"] as &[&str]),
                    ("categoryColors", &["object"] as &[&str]),
                    ("tooltipFrameUrl", &["asset"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("entity", &["string"] as &[&str]),
                    ("onItemDrop", &["function"] as &[&str]),
                    ("draggable", &["boolean"] as &[&str]),
                    ("onDragStart", &["function"] as &[&str]),
                    ("feedback", &["string"] as &[&str]),
                    ("onClick", &["function"] as &[&str]),
                    ("onRemove", &["function"] as &[&str]),
                    ("clickEvent", &["string"] as &[&str]),
                    ("removeEvent", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "trait-state-viewer",
                [
                    ("trait", &["object"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("showTransitions", &["boolean"] as &[&str]),
                    ("onStateClick", &["function"] as &[&str]),
                    ("stateStyles", &["object"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("entity", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "transition-arrow",
                [
                    ("from", &["object"] as &[&str]),
                    ("to", &["object"] as &[&str]),
                    ("eventLabel", &["string"] as &[&str]),
                    ("guardHint", &["string"] as &[&str]),
                    ("isActive", &["boolean"] as &[&str]),
                    ("onClick", &["function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "trend-indicator",
                [
                    ("className", &["string"] as &[&str]),
                    ("value", &["number"] as &[&str]),
                    ("direction", &["string"] as &[&str]),
                    ("showValue", &["boolean"] as &[&str]),
                    ("invert", &["boolean"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "turn-indicator",
                [
                    ("currentTurn", &["number"] as &[&str]),
                    ("maxTurns", &["number"] as &[&str]),
                    ("activeTeam", &["string"] as &[&str]),
                    ("phase", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "turn-panel",
                [
                    ("currentTurn", &["number"] as &[&str]),
                    ("maxTurns", &["number"] as &[&str]),
                    ("phase", &["string"] as &[&str]),
                    ("activeTeam", &["string"] as &[&str]),
                    ("actions", &["array"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "typewriter-text",
                [
                    ("text", &["string"] as &[&str]),
                    ("speed", &["number"] as &[&str]),
                    ("startDelay", &["number"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("onComplete", &["function"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "typography",
                [
                    ("variant", &["string"] as &[&str]),
                    ("level", &["number"] as &[&str]),
                    ("color", &["string"] as &[&str]),
                    ("align", &["string"] as &[&str]),
                    ("weight", &["string"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("truncate", &["boolean"] as &[&str]),
                    ("overflow", &["string"] as &[&str]),
                    ("as", &["component"] as &[&str]),
                    ("id", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("style", &["object"] as &[&str]),
                    ("content", &["node"] as &[&str]),
                    ("children", &["node"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "ui-slot-renderer",
                [
                    ("includeHud", &["boolean"] as &[&str]),
                    ("hudMode", &["string"] as &[&str]),
                    ("includeFloating", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("entity", &["string"] as &[&str]),
                    ("suspense", &["boolean", "object"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "uncontrolled-battle-board",
                [
                    ("entity", &["object", "array"] as &[&str]),
                    ("tiles", &["array"] as &[&str]),
                    ("units", &["array"] as &[&str]),
                    ("features", &["array"] as &[&str]),
                    ("assetManifest", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "unit-command-bar",
                [
                    ("commands", &["array"] as &[&str]),
                    ("selectedUnitId", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "upload-drop-zone",
                [
                    ("accept", &["string"] as &[&str]),
                    ("maxSize", &["number"] as &[&str]),
                    ("maxFiles", &["number"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                    ("description", &["string"] as &[&str]),
                    ("icon", &["icon", "string"] as &[&str]),
                    ("disabled", &["boolean"] as &[&str]),
                    ("action", &["string"] as &[&str]),
                    ("actionPayload", &["object"] as &[&str]),
                    ("onFiles", &["function"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "variable-panel",
                [
                    ("entityName", &["string"] as &[&str]),
                    ("variables", &["object", "array"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "version-diff",
                [
                    ("revisions", &["array", "object"] as &[&str]),
                    ("beforeId", &["string"] as &[&str]),
                    ("afterId", &["string"] as &[&str]),
                    ("view", &["string"] as &[&str]),
                    ("onSelectBefore", &["function"] as &[&str]),
                    ("onSelectAfter", &["function"] as &[&str]),
                    ("onRevert", &["function"] as &[&str]),
                    ("selectBeforeEvent", &["string"] as &[&str]),
                    ("selectAfterEvent", &["string"] as &[&str]),
                    ("revertEvent", &["string"] as &[&str]),
                    ("language", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "violation-alert",
                [
                    ("violation", &["object"] as &[&str]),
                    ("severity", &["string"] as &[&str]),
                    ("dismissible", &["boolean"] as &[&str]),
                    ("onDismiss", &["function"] as &[&str]),
                    ("onNavigateToField", &["function"] as &[&str]),
                    ("compact", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "vote-stack",
                [
                    ("count", &["number"] as &[&str]),
                    ("userVote", &["string"] as &[&str]),
                    ("onVote", &["function"] as &[&str]),
                    ("voteEvent", &["string"] as &[&str]),
                    ("disabled", &["boolean"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("variant", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("label", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "waypoint-marker",
                [
                    ("label", &["string"] as &[&str]),
                    ("icon", &["icon", "string"] as &[&str]),
                    ("active", &["boolean"] as &[&str]),
                    ("completed", &["boolean"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "wizard-container",
                [
                    ("steps", &["array"] as &[&str]),
                    ("currentStep", &["number", "string"] as &[&str]),
                    ("onStepChange", &["function"] as &[&str]),
                    ("onComplete", &["function"] as &[&str]),
                    ("showProgress", &["boolean"] as &[&str]),
                    ("allowBack", &["boolean"] as &[&str]),
                    ("compact", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("entity", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "wizard-navigation",
                [
                    ("currentStep", &["number"] as &[&str]),
                    ("totalSteps", &["number"] as &[&str]),
                    ("isValid", &["boolean"] as &[&str]),
                    ("showBack", &["boolean"] as &[&str]),
                    ("showNext", &["boolean"] as &[&str]),
                    ("showComplete", &["boolean"] as &[&str]),
                    ("backLabel", &["string"] as &[&str]),
                    ("nextLabel", &["string"] as &[&str]),
                    ("completeLabel", &["string"] as &[&str]),
                    ("onBack", &["string"] as &[&str]),
                    ("onNext", &["string"] as &[&str]),
                    ("onComplete", &["string"] as &[&str]),
                    ("onBackClick", &["function"] as &[&str]),
                    ("onNextClick", &["function"] as &[&str]),
                    ("onCompleteClick", &["function"] as &[&str]),
                    ("compact", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "wizard-progress",
                [
                    ("steps", &["array"] as &[&str]),
                    ("currentStep", &["number"] as &[&str]),
                    ("onStepClick", &["function"] as &[&str]),
                    ("allowNavigation", &["boolean"] as &[&str]),
                    ("compact", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("stepClickEvent", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "world-map-board",
                [
                    ("className", &["string"] as &[&str]),
                    ("isLoading", &["boolean"] as &[&str]),
                    ("error", &["object"] as &[&str]),
                    ("entity", &["object", "array"] as &[&str]),
                    ("tiles", &["array"] as &[&str]),
                    ("units", &["array"] as &[&str]),
                    ("features", &["array"] as &[&str]),
                    ("assetManifest", &["object"] as &[&str]),
                    ("scale", &["number"] as &[&str]),
                    ("unitScale", &["number"] as &[&str]),
                    ("allowMoveAllHeroes", &["boolean"] as &[&str]),
                    ("isInRange", &["function"] as &[&str]),
                    ("heroSelectEvent", &["string"] as &[&str]),
                    ("heroMoveEvent", &["string"] as &[&str]),
                    ("battleEncounterEvent", &["string"] as &[&str]),
                    ("featureEnterEvent", &["string"] as &[&str]),
                    ("tileClickEvent", &["string"] as &[&str]),
                    ("header", &["function"] as &[&str]),
                    ("sidePanel", &["function"] as &[&str]),
                    ("overlay", &["function"] as &[&str]),
                    ("footer", &["function"] as &[&str]),
                    ("onHeroSelect", &["function"] as &[&str]),
                    ("onHeroMove", &["function"] as &[&str]),
                    ("onBattleEncounter", &["function"] as &[&str]),
                    ("onFeatureEnter", &["function"] as &[&str]),
                    ("diamondTopY", &["number"] as &[&str]),
                    ("enableCamera", &["boolean"] as &[&str]),
                    ("effectSpriteUrls", &["array"] as &[&str]),
                    ("resolveUnitFrame", &["function"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "world-map-template",
                [
                    ("entity", &["object"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                    ("scale", &["number"] as &[&str]),
                    ("unitScale", &["number"] as &[&str]),
                    ("diamondTopY", &["number"] as &[&str]),
                    ("allowMoveAllHeroes", &["boolean"] as &[&str]),
                    ("tiles", &["array"] as &[&str]),
                    ("units", &["array"] as &[&str]),
                    ("features", &["array"] as &[&str]),
                    ("assetManifest", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "xp-bar",
                [
                    ("current", &["number"] as &[&str]),
                    ("max", &["number"] as &[&str]),
                    ("level", &["number"] as &[&str]),
                    ("showLabel", &["boolean"] as &[&str]),
                    ("size", &["string"] as &[&str]),
                    ("animated", &["boolean"] as &[&str]),
                    ("className", &["string"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
        ]
        .into_iter()
        .collect()
    });

/// Allowed string-literal values per pattern prop (pattern_type -> prop_name
/// -> allowed_values). Only present when the prop's declared TypeScript type
/// is a bounded union of string literals (e.g. StackJustify, AlertVariant).
/// The Rust validator rejects any literal value that isn't in this set.
pub static PATTERN_PROP_ENUM_VALUES: LazyLock<HashMap<&'static str, HashMap<&'static str, &'static [&'static str]>>> =
    LazyLock::new(|| {
        [
            (
                "action-button",
                [
                    ("size", &["sm", "md", "lg"] as &[&str]),
                    ("variant", &["primary", "secondary", "danger"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "action-buttons",
                [
                    ("layout", &["horizontal", "vertical", "diamond"] as &[&str]),
                    ("size", &["sm", "md", "lg"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("action-palette", [("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            ("action-tile", [("sortDirection", &["asc", "desc"] as &[&str]), ("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            ("alert", [("variant", &["info", "success", "warning", "error"] as &[&str])].into_iter().collect()),
            ("animated-graphic", [("animation", &["draw", "fill", "pulse", "morph"] as &[&str])].into_iter().collect()),
            (
                "animated-reveal",
                [
                    ("trigger", &["scroll", "hover", "manual"] as &[&str]),
                    ("animation", &["fade-up", "fade-down", "fade-in", "fade-left", "fade-right", "scale", "scale-up", "none"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("article-section", [("maxWidth", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            (
                "avatar",
                [
                    ("size", &["xs", "sm", "md", "lg", "xl"] as &[&str]),
                    ("status", &["online", "offline", "away", "busy"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "badge",
                [
                    ("variant", &["default", "primary", "secondary", "success", "warning", "danger", "error", "info", "neutral"] as &[&str]),
                    ("size", &["sm", "md", "lg"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("battle-board", [("sortDirection", &["asc", "desc"] as &[&str])].into_iter().collect()),
            ("book-chapter-view", [("direction", &["rtl", "ltr"] as &[&str])].into_iter().collect()),
            ("book-cover-page", [("direction", &["rtl", "ltr"] as &[&str])].into_iter().collect()),
            ("book-nav-bar", [("direction", &["rtl", "ltr"] as &[&str])].into_iter().collect()),
            ("book-table-of-contents", [("direction", &["rtl", "ltr"] as &[&str])].into_iter().collect()),
            ("book-viewer", [("sortDirection", &["asc", "desc"] as &[&str])].into_iter().collect()),
            (
                "box",
                [
                    ("padding", &["none", "xs", "sm", "md", "lg", "xl", "2xl"] as &[&str]),
                    ("paddingX", &["none", "xs", "sm", "md", "lg", "xl", "2xl"] as &[&str]),
                    ("paddingY", &["none", "xs", "sm", "md", "lg", "xl", "2xl"] as &[&str]),
                    ("margin", &["none", "xs", "sm", "md", "lg", "xl", "2xl", "auto"] as &[&str]),
                    ("marginX", &["none", "xs", "sm", "md", "lg", "xl", "2xl", "auto"] as &[&str]),
                    ("marginY", &["none", "xs", "sm", "md", "lg", "xl", "2xl", "auto"] as &[&str]),
                    ("bg", &["transparent", "primary", "secondary", "muted", "accent", "surface", "overlay"] as &[&str]),
                    ("rounded", &["none", "sm", "md", "lg", "xl", "2xl", "full"] as &[&str]),
                    ("shadow", &["none", "sm", "md", "lg", "xl"] as &[&str]),
                    ("display", &["block", "inline", "inline-block", "flex", "inline-flex", "grid"] as &[&str]),
                    ("overflow", &["auto", "hidden", "visible", "scroll"] as &[&str]),
                    ("position", &["relative", "absolute", "fixed", "sticky"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("builder-board", [("sortDirection", &["asc", "desc"] as &[&str])].into_iter().collect()),
            (
                "button",
                [
                    ("variant", &["primary", "secondary", "ghost", "danger", "success", "warning", "default"] as &[&str]),
                    ("size", &["sm", "md", "lg"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("calendar-grid", [("dayWindow", &["auto"] as &[&str])].into_iter().collect()),
            (
                "canvas-effect",
                [
                    ("actionType", &["melee", "ranged", "magic", "heal", "defend", "hit", "death", "buff", "debuff", "shield", "aoe", "critical"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "card",
                [
                    ("variant", &["default", "bordered", "elevated", "interactive"] as &[&str]),
                    ("padding", &["none", "sm", "md", "lg"] as &[&str]),
                    ("shadow", &["none", "sm", "md", "lg"] as &[&str]),
                    ("look", &["elevated", "flat-bordered", "borderless-divider", "ticket", "invoice", "chip", "tile-image-first"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("case-study-organism", [("sortDirection", &["asc", "desc"] as &[&str])].into_iter().collect()),
            (
                "chart",
                [
                    ("chartType", &["bar", "line", "pie", "area", "donut", "scatter", "histogram"] as &[&str]),
                    ("look", &["bar-vertical", "bar-horizontal", "line", "area", "pie", "donut", "scatter", "histogram"] as &[&str]),
                    ("stack", &["none", "stack", "normalize"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("chart-legend", [("direction", &["horizontal", "vertical"] as &[&str])].into_iter().collect()),
            ("classifier-board", [("sortDirection", &["asc", "desc"] as &[&str])].into_iter().collect()),
            (
                "code-block",
                [
                    ("language", &["text", "json", "javascript", "js", "typescript", "ts", "jsx", "tsx", "css", "markdown", "md", "bash", "shell", "sh", "yaml", "yml", "rust", "python", "py", "sql", "diff", "toml", "go", "graphql", "html", "xml", "orb", "lolo"] as &[&str]),
                    ("mode", &["code", "diff"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("combo-counter", [("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            (
                "confirm-dialog",
                [
                    ("variant", &["danger", "warning", "info", "default"] as &[&str]),
                    ("size", &["sm", "md", "lg", "xl", "full"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "container",
                [
                    ("size", &["xs", "sm", "md", "lg", "xl", "2xl", "full"] as &[&str]),
                    ("maxWidth", &["xs", "sm", "md", "lg", "xl", "2xl", "full"] as &[&str]),
                    ("padding", &["none", "sm", "md", "lg", "xl"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("content-renderer", [("sortDirection", &["asc", "desc"] as &[&str]), ("direction", &["rtl", "ltr"] as &[&str])].into_iter().collect()),
            (
                "content-section",
                [
                    ("background", &["default", "alt", "dark", "gradient"] as &[&str]),
                    ("padding", &["sm", "md", "lg"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "control-button",
                [
                    ("size", &["sm", "md", "lg", "xl"] as &[&str]),
                    ("shape", &["circle", "rounded", "square"] as &[&str]),
                    ("variant", &["primary", "secondary", "ghost"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("counter-template", [("size", &["sm", "md", "lg"] as &[&str]), ("variant", &["minimal", "standard", "full"] as &[&str])].into_iter().collect()),
            (
                "cta-banner",
                [
                    ("background", &["default", "alt", "dark", "gradient", "primary"] as &[&str]),
                    ("align", &["center", "left"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("d-pad", [("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            ("damage-number", [("type", &["damage", "heal", "crit", "miss"] as &[&str]), ("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            ("dashboard-grid", [("sortDirection", &["asc", "desc"] as &[&str]), ("gap", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            ("dashboard-layout", [("layoutMode", &["sidebar", "topnav", "bottomnav", "minimal"] as &[&str])].into_iter().collect()),
            (
                "data-grid",
                [
                    ("gap", &["none", "sm", "md", "lg", "xl"] as &[&str]),
                    ("look", &["dense", "spacious", "striped", "borderless", "card-rows"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "data-list",
                [
                    ("gap", &["none", "sm", "md", "lg"] as &[&str]),
                    ("variant", &["default", "card", "compact", "message"] as &[&str]),
                    ("look", &["dense", "spacious", "striped", "borderless", "card-rows"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("debugger-board", [("sortDirection", &["asc", "desc"] as &[&str])].into_iter().collect()),
            ("detail-panel", [("sortDirection", &["asc", "desc"] as &[&str]), ("position", &["left", "right"] as &[&str])].into_iter().collect()),
            ("dialogue-box", [("position", &["top", "bottom"] as &[&str])].into_iter().collect()),
            ("dialogue-bubble", [("position", &["top", "bottom"] as &[&str])].into_iter().collect()),
            (
                "divider",
                [
                    ("orientation", &["horizontal", "vertical"] as &[&str]),
                    ("variant", &["solid", "dashed", "dotted"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("document-viewer", [("documentType", &["pdf", "text", "html", "markdown"] as &[&str])].into_iter().collect()),
            (
                "drawer",
                [
                    ("position", &["left", "right"] as &[&str]),
                    ("width", &["sm", "md", "lg", "xl", "full"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("drawer-slot", [("position", &["left", "right"] as &[&str]), ("size", &["sm", "md", "lg", "xl", "full"] as &[&str])].into_iter().collect()),
            (
                "edge-decoration",
                [
                    ("variant", &["arch", "vine", "lattice"] as &[&str]),
                    ("side", &["left", "right", "both"] as &[&str]),
                    ("color", &["primary", "secondary", "success", "warning", "error", "muted"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "empty-state",
                [
                    ("variant", &["default", "success", "error", "warning", "info"] as &[&str]),
                    ("look", &["illustrated", "icon-only", "text-only", "mascot"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "entity-cards",
                [
                    ("sortDirection", &["asc", "desc"] as &[&str]),
                    ("gap", &["none", "sm", "md", "lg", "xl"] as &[&str]),
                    ("alignItems", &["start", "center", "end", "stretch"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("entity-list", [("sortDirection", &["asc", "desc"] as &[&str]), ("variant", &["default", "card"] as &[&str])].into_iter().collect()),
            (
                "entity-table",
                [
                    ("sortDirection", &["asc", "desc"] as &[&str]),
                    ("look", &["dense", "spacious", "striped", "borderless", "card-rows"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("event-handler-board", [("sortDirection", &["asc", "desc"] as &[&str])].into_iter().collect()),
            (
                "feature-card",
                [
                    ("variant", &["default", "bordered", "elevated", "interactive"] as &[&str]),
                    ("size", &["sm", "md", "lg"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("feature-grid", [("gap", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            ("feature-grid-organism", [("sortDirection", &["asc", "desc"] as &[&str])].into_iter().collect()),
            (
                "filter-group",
                [
                    ("variant", &["default", "compact", "pills", "vertical"] as &[&str]),
                    ("look", &["toolbar", "chips", "pills", "popover-trigger", "inline-column-header"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "filter-pill",
                [
                    ("variant", &["default", "primary", "secondary", "success", "warning", "danger", "info", "neutral"] as &[&str]),
                    ("size", &["sm", "md", "lg"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "flex",
                [
                    ("direction", &["row", "row-reverse", "col", "col-reverse"] as &[&str]),
                    ("wrap", &["nowrap", "wrap", "wrap-reverse"] as &[&str]),
                    ("align", &["start", "center", "end", "stretch", "baseline"] as &[&str]),
                    ("justify", &["start", "center", "end", "between", "around", "evenly"] as &[&str]),
                    ("gap", &["none", "xs", "sm", "md", "lg", "xl", "2xl"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "floating-action-button",
                [
                    ("variant", &["primary", "secondary", "success", "danger", "warning"] as &[&str]),
                    ("position", &["bottom-right", "bottom-left", "bottom-center", "top-right", "top-left", "top-center"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "form",
                [
                    ("layout", &["vertical", "horizontal", "inline"] as &[&str]),
                    ("gap", &["sm", "md", "lg"] as &[&str]),
                    ("mode", &["create", "edit"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "form-actions",
                [
                    ("variant", &["default", "segmented", "toggle"] as &[&str]),
                    ("orientation", &["horizontal", "vertical"] as &[&str]),
                    ("look", &["right-aligned-buttons", "floating-bar", "inline-row", "dropdown-menu", "command-palette-trigger"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "form-section",
                [
                    ("layout", &["vertical", "horizontal", "inline"] as &[&str]),
                    ("gap", &["sm", "md", "lg"] as &[&str]),
                    ("mode", &["create", "edit"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("form-section-header", [("badgeVariant", &["default", "primary", "success", "warning", "danger"] as &[&str])].into_iter().collect()),
            ("game-audio-toggle", [("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            (
                "game-canvas-3d",
                [
                    ("orientation", &["standard", "rotated"] as &[&str]),
                    ("cameraMode", &["isometric", "perspective", "top-down"] as &[&str]),
                    ("overlay", &["default", "hidden", "minimap"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("game-canvas-3d-battle-template", [("cameraMode", &["isometric", "perspective", "top-down"] as &[&str])].into_iter().collect()),
            ("game-canvas-3d-castle-template", [("cameraMode", &["isometric", "perspective", "top-down"] as &[&str])].into_iter().collect()),
            ("game-canvas-3d-world-map-template", [("cameraMode", &["isometric", "perspective", "top-down"] as &[&str])].into_iter().collect()),
            ("game-hud", [("position", &["top", "bottom", "corners"] as &[&str]), ("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            ("game-over-screen", [("variant", &["victory", "defeat", "neutral"] as &[&str])].into_iter().collect()),
            (
                "geometric-pattern",
                [
                    ("variant", &["star8", "star6", "khatam", "star10", "star12", "rosette-double", "rosette-filled", "seigaiha", "greek-key", "celtic-knot", "kolam", "arch", "arabesque-vine", "arabesque-net"] as &[&str]),
                    ("mode", &["background", "left", "right", "dual", "around", "frame"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("gradient-divider", [("color", &["primary", "secondary", "success", "warning", "error", "muted"] as &[&str])].into_iter().collect()),
            ("graph-canvas", [("layout", &["force", "circular", "grid"] as &[&str])].into_iter().collect()),
            (
                "grid",
                [
                    ("cols", &["none"] as &[&str]),
                    ("gap", &["none", "xs", "sm", "md", "lg", "xl", "2xl"] as &[&str]),
                    ("rowGap", &["none", "xs", "sm", "md", "lg", "xl", "2xl"] as &[&str]),
                    ("colGap", &["none", "xs", "sm", "md", "lg", "xl", "2xl"] as &[&str]),
                    ("alignItems", &["start", "center", "end", "stretch", "baseline"] as &[&str]),
                    ("justifyItems", &["start", "center", "end", "stretch"] as &[&str]),
                    ("flow", &["row", "col", "row-dense", "col-dense"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "header",
                [
                    ("variant", &["mobile", "desktop"] as &[&str]),
                    ("look", &["hero", "compact-bar", "breadcrumb", "contextual", "editorial-banner"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("health-bar", [("format", &["hearts", "bar", "numeric"] as &[&str]), ("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            ("health-panel", [("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            ("hero-organism", [("sortDirection", &["asc", "desc"] as &[&str])].into_iter().collect()),
            (
                "hero-section",
                [
                    ("tagVariant", &["primary", "secondary", "accent"] as &[&str]),
                    ("imagePosition", &["below", "right", "background"] as &[&str]),
                    ("background", &["dark", "gradient", "subtle"] as &[&str]),
                    ("align", &["center", "left"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "icon",
                [
                    ("size", &["xs", "sm", "md", "lg", "xl"] as &[&str]),
                    ("color", &["primary", "secondary", "success", "warning", "error", "muted"] as &[&str]),
                    ("animation", &["spin", "pulse", "none"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "input",
                [
                    ("inputType", &["text", "email", "password", "number", "tel", "url", "search", "date", "datetime-local", "time", "checkbox", "select", "textarea"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("inventory-grid", [("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            (
                "item-slot",
                [
                    ("rarity", &["common", "uncommon", "rare", "epic", "legendary"] as &[&str]),
                    ("size", &["sm", "md", "lg"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("jazari-state-machine", [("direction", &["ltr", "rtl"] as &[&str])].into_iter().collect()),
            ("landing-page-template", [("variant", &["product", "service", "platform"] as &[&str])].into_iter().collect()),
            ("law-reference-tooltip", [("position", &["top", "bottom", "left", "right"] as &[&str])].into_iter().collect()),
            ("likert-scale", [("size", &["sm", "md", "lg"] as &[&str]), ("variant", &["radios", "buttons"] as &[&str])].into_iter().collect()),
            ("markdown-content", [("direction", &["ltr", "rtl"] as &[&str])].into_iter().collect()),
            ("master-detail", [("sortDirection", &["asc", "desc"] as &[&str])].into_iter().collect()),
            ("matrix-question", [("size", &["sm", "md"] as &[&str])].into_iter().collect()),
            (
                "media-gallery",
                [
                    ("sortDirection", &["asc", "desc"] as &[&str]),
                    ("aspectRatio", &["square", "landscape", "portrait"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "menu",
                [
                    ("position", &["top-left", "top-right", "bottom-left", "bottom-right", "top-start", "top-end", "bottom-start", "bottom-end"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "meter",
                [
                    ("variant", &["linear", "radial", "segmented"] as &[&str]),
                    ("size", &["sm", "md", "lg"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "modal",
                [
                    ("size", &["sm", "md", "lg", "xl", "full"] as &[&str]),
                    ("look", &["centered-card", "top-sheet", "side-drawer", "full-screen"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("modal-slot", [("size", &["sm", "md", "lg", "xl", "full"] as &[&str])].into_iter().collect()),
            ("navigation", [("orientation", &["horizontal", "vertical"] as &[&str])].into_iter().collect()),
            ("negotiator-board", [("sortDirection", &["asc", "desc"] as &[&str])].into_iter().collect()),
            ("notification", [("variant", &["success", "error", "info", "warning"] as &[&str])].into_iter().collect()),
            ("number-stepper", [("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            ("option-constraint-group", [("size", &["sm", "md"] as &[&str])].into_iter().collect()),
            ("orbital-visualization", [("size", &["sm", "md", "lg", "xl"] as &[&str])].into_iter().collect()),
            (
                "pattern-tile",
                [
                    ("variant", &["star8", "star6", "khatam", "star10", "star12", "rosette-double", "rosette-filled", "seigaiha", "greek-key", "celtic-knot", "kolam", "arch", "arabesque-vine", "arabesque-net"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "popover",
                [
                    ("position", &["top", "bottom", "left", "right"] as &[&str]),
                    ("trigger", &["click", "hover"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("pricing-organism", [("sortDirection", &["asc", "desc"] as &[&str])].into_iter().collect()),
            (
                "progress-bar",
                [
                    ("progressType", &["linear", "circular", "stepped"] as &[&str]),
                    ("variant", &["default", "primary", "success", "warning", "error", "danger"] as &[&str]),
                    ("color", &["default", "primary", "success", "warning", "error", "danger"] as &[&str]),
                    ("size", &["sm", "md", "lg"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("progress-dots", [("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            ("qr-scanner", [("facingMode", &["environment", "user"] as &[&str])].into_iter().collect()),
            ("radio", [("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            ("range-slider", [("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            ("resource-bar", [("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            (
                "resource-counter",
                [
                    ("color", &["primary", "secondary", "success", "warning", "error", "muted"] as &[&str]),
                    ("size", &["sm", "md", "lg"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "runtime-debugger",
                [
                    ("position", &["bottom-right", "bottom-left", "top-right", "top-left"] as &[&str]),
                    ("mode", &["floating", "inline", "verify"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("score-display", [("size", &["sm", "md", "lg", "xl"] as &[&str])].into_iter().collect()),
            (
                "section",
                [
                    ("padding", &["none", "sm", "md", "lg", "xl"] as &[&str]),
                    ("variant", &["default", "card", "bordered", "filled"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("section-header", [("align", &["center", "left", "right"] as &[&str])].into_iter().collect()),
            ("sequence-bar", [("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            ("sequencer-board", [("sortDirection", &["asc", "desc"] as &[&str])].into_iter().collect()),
            ("showcase-organism", [("sortDirection", &["asc", "desc"] as &[&str])].into_iter().collect()),
            ("side-panel", [("position", &["left", "right"] as &[&str])].into_iter().collect()),
            ("simple-grid", [("gap", &["none", "xs", "sm", "md", "lg", "xl"] as &[&str])].into_iter().collect()),
            ("simulator-board", [("sortDirection", &["asc", "desc"] as &[&str])].into_iter().collect()),
            ("skeleton", [("variant", &["header", "table", "form", "card", "text"] as &[&str])].into_iter().collect()),
            ("social-proof", [("variant", &["logos", "quotes", "badges"] as &[&str])].into_iter().collect()),
            ("sortable-list", [("dragHandlePosition", &["left", "right"] as &[&str])].into_iter().collect()),
            (
                "spacer",
                [
                    ("size", &["xs", "sm", "md", "lg", "xl", "2xl", "auto"] as &[&str]),
                    ("axis", &["horizontal", "vertical"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("sparkline", [("color", &["auto", "primary", "success", "warning", "error", "info", "muted"] as &[&str])].into_iter().collect()),
            ("spinner", [("size", &["xs", "sm", "md", "lg"] as &[&str])].into_iter().collect()),
            (
                "split",
                [
                    ("ratio", &["1:1", "1:2", "2:1", "1:3", "3:1", "1:4", "4:1", "2:3", "3:2"] as &[&str]),
                    ("gap", &["none", "sm", "md", "lg", "xl"] as &[&str]),
                    ("stackBreakpoint", &["sm", "md", "lg", "xl"] as &[&str]),
                    ("align", &["start", "center", "end", "stretch"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("split-pane", [("direction", &["horizontal", "vertical"] as &[&str])].into_iter().collect()),
            ("split-section", [("imagePosition", &["left", "right"] as &[&str]), ("background", &["default", "alt"] as &[&str])].into_iter().collect()),
            (
                "stack",
                [
                    ("direction", &["horizontal", "vertical"] as &[&str]),
                    ("gap", &["none", "xs", "sm", "md", "lg", "xl", "2xl"] as &[&str]),
                    ("align", &["start", "center", "end", "stretch", "baseline"] as &[&str]),
                    ("justify", &["start", "center", "end", "between", "around", "evenly"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("star-rating", [("precision", &["full", "half"] as &[&str]), ("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            (
                "stat-badge",
                [
                    ("format", &["number", "hearts", "bar", "text"] as &[&str]),
                    ("size", &["sm", "md", "lg"] as &[&str]),
                    ("variant", &["default", "primary", "success", "warning", "danger"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("stat-card", [("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            (
                "stat-display",
                [
                    ("trendPolarity", &["higher-is-better", "lower-is-better"] as &[&str]),
                    ("trendFormat", &["absolute", "percent"] as &[&str]),
                    ("size", &["sm", "md", "lg"] as &[&str]),
                    ("variant", &["default", "primary", "success", "warning", "error", "info"] as &[&str]),
                    ("look", &["elevated", "flat", "progress-backed", "gauge", "sparkline"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("state-architect-board", [("sortDirection", &["asc", "desc"] as &[&str])].into_iter().collect()),
            ("state-indicator", [("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            ("stats-organism", [("sortDirection", &["asc", "desc"] as &[&str])].into_iter().collect()),
            (
                "status-dot",
                [
                    ("status", &["online", "offline", "away", "busy", "warning", "critical"] as &[&str]),
                    ("size", &["sm", "md", "lg"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("status-effect", [("variant", &["buff", "debuff", "neutral"] as &[&str]), ("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            ("step-flow", [("orientation", &["horizontal", "vertical"] as &[&str])].into_iter().collect()),
            (
                "step-flow-organism",
                [
                    ("sortDirection", &["asc", "desc"] as &[&str]),
                    ("orientation", &["horizontal", "vertical"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("svg-branch", [("variant", &["fork", "merge", "diamond"] as &[&str])].into_iter().collect()),
            ("svg-connection", [("variant", &["solid", "dashed", "animated"] as &[&str])].into_iter().collect()),
            ("svg-morph", [("variant", &["text-to-code", "code-to-app", "generic"] as &[&str])].into_iter().collect()),
            ("svg-node", [("variant", &["filled", "stroked", "pulse"] as &[&str])].into_iter().collect()),
            ("svg-ring", [("variant", &["solid", "dashed", "glow"] as &[&str])].into_iter().collect()),
            ("svg-shield", [("variant", &["outline", "filled", "check"] as &[&str])].into_iter().collect()),
            ("tabbed-container", [("position", &["top", "left"] as &[&str])].into_iter().collect()),
            (
                "table-view",
                [
                    ("sortDirection", &["asc", "desc"] as &[&str]),
                    ("look", &["dense", "spacious", "striped", "borderless", "bordered"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            (
                "tabs",
                [
                    ("variant", &["default", "pills", "underline"] as &[&str]),
                    ("orientation", &["horizontal", "vertical"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("tag-cloud", [("variant", &["default", "primary", "accent"] as &[&str])].into_iter().collect()),
            (
                "tag-input",
                [
                    ("variant", &["default", "primary", "secondary", "success", "warning", "danger", "error", "info", "neutral"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("team-organism", [("sortDirection", &["asc", "desc"] as &[&str])].into_iter().collect()),
            ("text-highlight", [("highlightType", &["question", "note"] as &[&str])].into_iter().collect()),
            ("theme-toggle", [("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            ("timeline", [("look", &["vertical-compact", "vertical-spacious", "horizontal", "swimlane"] as &[&str])].into_iter().collect()),
            ("timer-display", [("format", &["mm:ss", "ss", "countdown"] as &[&str]), ("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            ("toast-slot", [("variant", &["success", "error", "info", "warning"] as &[&str])].into_iter().collect()),
            ("tooltip", [("position", &["top", "bottom", "left", "right"] as &[&str])].into_iter().collect()),
            ("trait-slot", [("size", &["sm", "md", "lg"] as &[&str]), ("feedback", &["correct", "wrong"] as &[&str])].into_iter().collect()),
            ("trait-state-viewer", [("variant", &["linear", "compact", "full"] as &[&str]), ("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            ("trend-indicator", [("direction", &["up", "down", "flat"] as &[&str]), ("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            ("turn-indicator", [("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            (
                "typography",
                [
                    ("variant", &["h1", "h2", "h3", "h4", "h5", "h6", "heading", "subheading", "body1", "body2", "body", "caption", "overline", "small", "large", "label"] as &[&str]),
                    ("color", &["primary", "secondary", "muted", "error", "success", "warning", "inherit"] as &[&str]),
                    ("align", &["left", "center", "right"] as &[&str]),
                    ("weight", &["light", "normal", "medium", "semibold", "bold"] as &[&str]),
                    ("size", &["xs", "sm", "md", "lg", "xl", "2xl", "3xl"] as &[&str]),
                    ("overflow", &["visible", "hidden", "wrap", "clamp-2", "clamp-3"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("ui-slot-renderer", [("hudMode", &["fixed", "inline"] as &[&str])].into_iter().collect()),
            ("version-diff", [("view", &["side-by-side", "inline"] as &[&str])].into_iter().collect()),
            ("violation-alert", [("severity", &["warning", "error"] as &[&str])].into_iter().collect()),
            (
                "vote-stack",
                [
                    ("userVote", &["up", "down", "none"] as &[&str]),
                    ("size", &["sm", "md", "lg"] as &[&str]),
                    ("variant", &["vertical", "horizontal"] as &[&str]),
                ]
                .into_iter()
                .collect(),
            ),
            ("waypoint-marker", [("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
            ("xp-bar", [("size", &["sm", "md", "lg"] as &[&str])].into_iter().collect()),
        ]
        .into_iter()
        .collect()
    });

/// Event-valued prop names per pattern (pattern_type -> set of prop names
/// whose TypeScript type is EventKey from @almadar/core).
///
/// The inline phase consults this set when walking an SExpression::Object
/// with a known 'type' field: for every object key in this set, the prop's
/// string value flows through event_subs (the trait-ref events rename
/// map) so call-site event renames cascade through render-ui trees.
///
/// Derived from pattern-sync's type-based detection — no name heuristics.
pub static PATTERN_EVENT_PROPS: LazyLock<HashMap<&'static str, HashSet<&'static str>>> =
    LazyLock::new(|| {
        [
            ("alert", ["dismissEvent"].into_iter().collect()),
            ("avatar", ["action"].into_iter().collect()),
            ("box", ["action", "hoverEvent"].into_iter().collect()),
            ("button", ["action"].into_iter().collect()),
            ("card", ["action"].into_iter().collect()),
            ("control-button", ["pressEvent", "releaseEvent"].into_iter().collect()),
            ("data-grid", ["selectionEvent", "loadMoreEvent"].into_iter().collect()),
            ("data-list", ["reorderEvent", "itemClickEvent", "swipeLeftEvent", "swipeRightEvent", "longPressEvent", "loadMoreEvent"].into_iter().collect()),
            ("drawer", ["closeEvent"].into_iter().collect()),
            ("floating-action-button", ["action"].into_iter().collect()),
            ("form", ["submitEvent", "cancelEvent"].into_iter().collect()),
            ("form-section", ["submitEvent", "cancelEvent"].into_iter().collect()),
            ("infinite-scroll-sentinel", ["loadMoreEvent"].into_iter().collect()),
            ("input", ["action"].into_iter().collect()),
            ("number-stepper", ["action"].into_iter().collect()),
            ("overlay", ["action"].into_iter().collect()),
            ("pull-to-refresh", ["refreshEvent"].into_iter().collect()),
            ("radio", ["action"].into_iter().collect()),
            ("range-slider", ["action"].into_iter().collect()),
            ("select", ["action"].into_iter().collect()),
            ("sortable-list", ["reorderEvent"].into_iter().collect()),
            ("sprite", ["action"].into_iter().collect()),
            ("stack", ["action"].into_iter().collect()),
            ("star-rating", ["action"].into_iter().collect()),
            ("table-view", ["selectEvent", "sortEvent"].into_iter().collect()),
            ("tag-input", ["addEvent", "removeEvent"].into_iter().collect()),
            ("text-highlight", ["action", "hoverEvent"].into_iter().collect()),
            ("textarea", ["action"].into_iter().collect()),
            ("upload-drop-zone", ["action"].into_iter().collect()),
        ]
        .into_iter()
        .collect()
    });

/// Event-list props per pattern (pattern_type -> prop_name -> eventField).
///
/// When a pattern declares a prop as event-list (array of descriptor
/// objects, each with a field like 'event' carrying a declared event key),
/// the inline phase iterates the array and rewrites each item's
/// eventField-named string value via event_subs.
///
/// Default eventField is 'event'; patterns with a non-default field
/// surface the name explicitly.
pub static PATTERN_EVENT_LIST_PROPS: LazyLock<HashMap<&'static str, HashMap<&'static str, &'static str>>> =
    LazyLock::new(|| {
        [
            ("dashboard-layout", [("topBarActions", "event")].into_iter().collect()),
            ("data-grid", [("itemActions", "event")].into_iter().collect()),
            ("data-list", [("itemActions", "event")].into_iter().collect()),
            ("entity-cards", [("itemActions", "event")].into_iter().collect()),
            ("swipeable-row", [("leftActions", "event"), ("rightActions", "event")].into_iter().collect()),
            ("table-view", [("itemActions", "event")].into_iter().collect()),
            ("timeline", [("itemActions", "event")].into_iter().collect()),
        ]
        .into_iter()
        .collect()
    });

/// Patterns that require action/event props
pub static ACTION_REQUIRED_PATTERNS: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| ["button", "icon-button", "link"].into_iter().collect());

/// Check if a pattern type is valid
pub fn is_valid_pattern(pattern_type: &str) -> bool {
    PATTERN_TYPES.contains(pattern_type)
}

/// Get the category for a pattern
pub fn get_pattern_category(pattern_type: &str) -> Option<&'static str> {
    PATTERN_TO_CATEGORY.get(pattern_type).copied()
}

/// Get required props for a pattern
pub fn get_pattern_props(pattern_type: &str) -> Option<&'static Vec<&'static str>> {
    PATTERN_REQUIRED_PROPS.get(pattern_type)
}

/// Check if a pattern requires action prop
pub fn requires_action(pattern_type: &str) -> bool {
    ACTION_REQUIRED_PATTERNS.contains(pattern_type)
}

/// Check if an itemAction prop is valid
pub fn is_valid_item_action_prop(prop: &str) -> bool {
    VALID_ITEM_ACTION_PROPS.contains(prop)
}

/// Get valid props for a pattern (from PATTERN_PROPS_SCHEMA)
pub fn get_pattern_valid_props(pattern_type: &str) -> Option<&'static HashSet<&'static str>> {
    PATTERN_PROPS_SCHEMA.get(pattern_type)
}

/// Get expected types for a specific prop on a pattern.
/// Returns None if the pattern or prop is not in the registry, or if types are unknown.
pub fn get_prop_types(pattern_type: &str, prop_name: &str) -> Option<&'static [&'static str]> {
    PATTERN_PROP_TYPES
        .get(pattern_type)
        .and_then(|props| props.get(prop_name))
        .copied()
}

/// Get the allowed string-literal enum values for a prop on a pattern.
/// Returns Some(&["start","center",...]) when the prop's TS type is a
/// bounded union of string literals (e.g. StackJustify); None when the
/// prop is unbounded (plain string) or the pattern/prop is unknown.
pub fn get_prop_enum_values(
    pattern_type: &str,
    prop_name: &str,
) -> Option<&'static [&'static str]> {
    PATTERN_PROP_ENUM_VALUES
        .get(pattern_type)
        .and_then(|props| props.get(prop_name))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pattern_types() {
        assert!(is_valid_pattern("entity-table"));
        assert!(is_valid_pattern("form"));
        assert!(is_valid_pattern("game-canvas-2d"));
        assert!(!is_valid_pattern("nonexistent-pattern"));
    }

    #[test]
    fn test_pattern_category() {
        assert_eq!(get_pattern_category("entity-table"), Some("display"));
        assert_eq!(get_pattern_category("form"), Some("form"));
        assert_eq!(get_pattern_category("game-canvas-2d"), Some("game"));
        assert_eq!(get_pattern_category("nonexistent"), None);
    }

    #[test]
    fn test_item_action_props() {
        assert!(is_valid_item_action_prop("label"));
        assert!(is_valid_item_action_prop("event"));
        assert!(!is_valid_item_action_prop("condition"));
    }

    #[test]
    fn test_all_patterns_have_category() {
        for pattern in PATTERN_TYPES.iter() {
            assert!(
                PATTERN_TO_CATEGORY.contains_key(pattern),
                "Pattern '{}' missing from PATTERN_TO_CATEGORY",
                pattern
            );
        }
    }

    #[test]
    fn test_all_categorized_patterns_are_valid() {
        for (pattern, _) in PATTERN_TO_CATEGORY.iter() {
            assert!(
                PATTERN_TYPES.contains(pattern),
                "Pattern '{}' in PATTERN_TO_CATEGORY but not in PATTERN_TYPES",
                pattern
            );
        }
    }
}
