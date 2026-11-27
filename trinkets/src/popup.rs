//! Where the inventory popup's slots go: pure geometry, no clicks and no drawing. The popup is drawn over the
//! player's inventory screen today; once the slots are real ones of the inventory menu (sunrise-org/sunrise#4980)
//! this layout and the sprites stay, and only the click path changes.
use std::sync::LazyLock;

use crate::data::{DISPLAY, SLOTS};

/// A group of `SLOTS` (the first part of a slot id): its id, where its first slot sits in the inventory panel
/// (the item's top left), and, for a group over a vanilla slot, that slot's menu index and empty-slot sprite.
pub struct Group {
    pub id: &'static str,
    pub at: [f32; 2],
    pub vanilla: Option<(usize, &'static str)>,
}

/// The groups in the original's order: over the armour and offhand slots, then (as the wiki says) "above the offhand
/// going up", the groups that have no vanilla slot, whose first slot is always shown.
pub const GROUPS: &[Group] = &[
    Group {
        id: "head",
        at: [8.0, 8.0],
        vanilla: Some((5, "container/slot/helmet")),
    },
    Group {
        id: "chest",
        at: [8.0, 26.0],
        vanilla: Some((6, "container/slot/chestplate")),
    },
    Group {
        id: "legs",
        at: [8.0, 44.0],
        vanilla: Some((7, "container/slot/leggings")),
    },
    Group {
        id: "feet",
        at: [8.0, 62.0],
        vanilla: Some((8, "container/slot/boots")),
    },
    Group {
        id: "offhand",
        at: [77.0, 62.0],
        vanilla: Some((45, "container/slot/shield")),
    },
    Group {
        id: "hand",
        at: [77.0, 44.0],
        vanilla: None,
    },
    Group {
        id: "charm",
        at: [77.0, 26.0],
        vanilla: None,
    },
];
/// A slot's pitch.
pub const CELL: f32 = 18.0;
/// The popup frame's pieces (`more_slots.png`): a cap each side and the stretch behind one slot, 26 high.
pub const FRAME_H: f32 = 26.0;
pub const FRAME_CAP: f32 = 4.0;

/// The indices of `SLOTS` in each group, in display order. Worked out once.
static MEMBERS: LazyLock<Vec<Vec<usize>>> = LazyLock::new(|| {
    GROUPS
        .iter()
        .map(|g| {
            let prefix = format!("{}/", g.id);
            DISPLAY
                .iter()
                .filter(|d| d.starts_with(&prefix))
                .filter_map(|d| SLOTS.iter().position(|s| s.0 == *d))
                .collect()
        })
        .collect()
});

/// The indices of `SLOTS` in group `g`, in the order the popup shows them (not the equip order).
pub fn members(g: usize) -> &'static [usize] {
    &MEMBERS[g]
}

/// Where position `n` of a group sits from the first: the wiki's `8 6 4 2 0 1 3 5 7 9`.
pub fn offset(n: usize) -> i32 {
    if n % 2 == 1 {
        (n as i32 + 1) / 2
    } else {
        -(n as i32 / 2)
    }
}

pub fn inside(rect: [f32; 4], p: [f32; 2]) -> bool {
    p[0] >= rect[0] && p[0] < rect[0] + rect[2] && p[1] >= rect[1] && p[1] < rect[1] + rect[3]
}

/// A group's slots and frame relative to the panel's top left, for one direction of the row.
pub struct Shape {
    cells: Vec<(usize, [f32; 2])>,
    frame: [f32; 4],
}

/// Every group's shape, normal and mirrored (the row growing left instead of right), worked out once.
static SHAPES: LazyLock<Vec<[Shape; 2]>> = LazyLock::new(|| {
    (0..GROUPS.len())
        .map(|g| [shape(g, 1.0), shape(g, -1.0)])
        .collect()
});

/// A vanilla slot is position 0 and the group's slots follow it. The original has no slot type with several
/// copies in this port, so there is no second row (its vertical expansion) to lay out.
fn shape(g: usize, sign: f32) -> Shape {
    let group = &GROUPS[g];
    let start = usize::from(group.vanilla.is_some());
    let cells: Vec<_> = members(g)
        .iter()
        .enumerate()
        .map(|(k, &i)| {
            let x = group.at[0] + sign * offset(k + start) as f32 * CELL;
            (i, [x, group.at[1]])
        })
        .collect();
    let (mut lo, mut hi) = (0, 0);
    for k in 0..cells.len() + start {
        let o = (sign * offset(k) as f32) as i32;
        lo = lo.min(o);
        hi = hi.max(o);
    }
    let x = group.at[0] + lo as f32 * CELL - 1.0 - FRAME_CAP;
    let w = (hi - lo + 1) as f32 * CELL + 2.0 * FRAME_CAP;
    Shape {
        cells,
        frame: [x, group.at[1] - 5.0, w, FRAME_H],
    }
}

/// A group placed on a screen: its shape moved to the panel, mirrored or pushed back when it would leave the screen.
pub struct Placed {
    shape: &'static Shape,
    origin: [f32; 2],
}

impl Placed {
    /// The item top left of each slot (slot index, x, y).
    pub fn cells(&self) -> impl Iterator<Item = (usize, [f32; 2])> + '_ {
        let o = self.origin;
        self.shape
            .cells
            .iter()
            .map(move |&(i, at)| (i, [o[0] + at[0], o[1] + at[1]]))
    }

    /// The popup frame `[x, y, w, h]` round the row.
    pub fn frame(&self) -> [f32; 4] {
        let f = self.shape.frame;
        [self.origin[0] + f[0], self.origin[1] + f[1], f[2], f[3]]
    }

    /// The item top left of the group's first position.
    pub fn first(&self, g: usize) -> [f32; 2] {
        [
            self.origin[0] + GROUPS[g].at[0],
            self.origin[1] + GROUPS[g].at[1],
        ]
    }
}

/// How far to move `[from, from + len]` to lie within `[0, room]` (it goes to 0 when it is longer than the room).
fn pull(from: f32, len: f32, room: f32) -> f32 {
    if from < 0.0 {
        -from
    } else if from + len > room {
        (room - from - len).max(-from)
    } else {
        0.0
    }
}

/// Group `g` on the inventory panel `panel` of a screen `screen` (GUI pixels) wide and high: as the original's, the
/// row goes the other way when it would leave the screen on one side and fits on the other, else it is pushed back in.
pub fn place(panel: [f32; 4], screen: [f32; 2], g: usize) -> Placed {
    let [normal, mirrored] = &SHAPES[g];
    let fits = |s: &Shape| {
        let x = panel[0] + s.frame[0];
        x >= 0.0 && x + s.frame[2] <= screen[0]
    };
    let shape = if !fits(normal) && fits(mirrored) {
        mirrored
    } else {
        normal
    };
    let f = shape.frame;
    Placed {
        shape,
        origin: [
            panel[0] + pull(panel[0] + f[0], f[2], screen[0]),
            panel[1] + pull(panel[1] + f[1], f[3], screen[1]),
        ],
    }
}

/// The 18 pixel box of group `g`'s first position (the vanilla slot, or its first slot).
pub fn anchor_cell(panel: [f32; 4], g: usize) -> [f32; 4] {
    [
        panel[0] + GROUPS[g].at[0] - 1.0,
        panel[1] + GROUPS[g].at[1] - 1.0,
        CELL,
        CELL,
    ]
}

/// The group whose popup is open: the one whose first position the pointer is on, else the open one (`current`)
/// while the pointer stays in its frame.
pub fn open_group(
    panel: [f32; 4],
    screen: [f32; 2],
    mouse: [f32; 2],
    current: Option<usize>,
) -> Option<usize> {
    (0..GROUPS.len())
        .find(|&g| inside(anchor_cell(panel, g), mouse))
        .or(current.filter(|&g| inside(place(panel, screen, g).frame(), mouse)))
}

/// The trinket slot of the open group `open` the pointer is on.
pub fn over(
    panel: [f32; 4],
    screen: [f32; 2],
    mouse: [f32; 2],
    open: Option<usize>,
) -> Option<usize> {
    place(panel, screen, open?)
        .cells()
        .find(|(_, at)| inside([at[0] - 1.0, at[1] - 1.0, CELL, CELL], mouse))
        .map(|(i, _)| i)
}
