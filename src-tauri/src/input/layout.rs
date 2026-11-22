// Trovul - Keyboard & mouse sharing: the shared screen arrangement (the
// "mosaic" the user draws) and the pointer's path through it. Pure geometry,
// no Windows calls, so every decision is unit-tested.
//
// Each monitor has two rectangles:
//   native — where its own PC's Windows puts it (virtual-desktop pixels);
//   world  — where the user placed it in the shared arrangement.
// Movement *within* a monitor is left to Windows. Crossing a monitor edge is
// decided by the world arrangement, so PC2's screen can sit between PC1's two
// screens even though Windows thinks PC1's screens touch each other.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(&self) -> i32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.height
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }

    pub fn clamp(&self, p: Point) -> Point {
        Point::new(
            p.x.clamp(self.x, self.right() - 1),
            p.y.clamp(self.y, self.bottom() - 1),
        )
    }

    pub fn center(&self) -> Point {
        Point::new(self.x + self.width / 2, self.y + self.height / 2)
    }

    fn overlaps(&self, other: &Rect) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }
}

/// A monitor as its own PC reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Monitor {
    /// Stable id (the monitor's device path, else its GDI name).
    pub id: String,
    /// Windows' display number ("1" in Settings → Display).
    pub number: u32,
    /// Friendly name, e.g. "LG ULTRAGEAR".
    pub name: String,
    /// Native rectangle, in physical pixels.
    pub rect: Rect,
    pub primary: bool,
}

/// Top-left corner of one monitor in the shared arrangement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    /// Device id of the PC the monitor belongs to.
    pub pc: String,
    pub monitor: String,
    pub x: i32,
    pub y: i32,
}

/// The arrangement both PCs share; the newest `updated` wins when they sync.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SharedLayout {
    /// Wall-clock ms of the last edit (0 = never edited).
    pub updated: u64,
    pub placements: Vec<Placement>,
}

/// One PC's monitors, as input to `World::build`.
pub struct PcScreens<'a> {
    pub pc: &'a str,
    pub monitors: &'a [Monitor],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub pc: String,
    /// Belongs to this PC (as opposed to the linked one).
    pub local: bool,
    pub monitor: Monitor,
    pub world: Rect,
}

impl Screen {
    pub fn native(&self) -> Rect {
        self.monitor.rect
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct World {
    pub screens: Vec<Screen>,
}

impl World {
    /// Arrange both PCs' monitors. Saved placements win; monitors without one
    /// get the default: the PC with the smaller device id keeps its Windows
    /// arrangement and the other PC sits to its right, tops aligned. Both PCs
    /// compute the same default, so they agree before anyone edits anything.
    pub fn build(local: PcScreens, remote: PcScreens, placements: &[Placement]) -> World {
        let (first, second) = if local.pc <= remote.pc {
            (&local, &remote)
        } else {
            (&remote, &local)
        };
        let first_box = bounding_box(first.monitors.iter().map(|m| m.rect));
        let second_box = bounding_box(second.monitors.iter().map(|m| m.rect));
        let offset = match (first_box, second_box) {
            (Some(a), Some(b)) => Point::new(a.right() - b.x, a.y - b.y),
            _ => Point::default(),
        };

        let mut screens: Vec<Screen> = Vec::new();
        let mut unplaced: Vec<Screen> = Vec::new();
        for (pcs, shift) in [(first, Point::default()), (second, offset)] {
            for monitor in pcs.monitors {
                let saved = placements
                    .iter()
                    .find(|p| p.pc == pcs.pc && p.monitor == monitor.id);
                let origin = saved.map_or(
                    Point::new(monitor.rect.x + shift.x, monitor.rect.y + shift.y),
                    |p| Point::new(p.x, p.y),
                );
                let screen = Screen {
                    pc: pcs.pc.to_string(),
                    local: pcs.pc == local.pc,
                    monitor: monitor.clone(),
                    world: Rect::new(origin.x, origin.y, monitor.rect.width, monitor.rect.height),
                };
                if saved.is_some() || placements.is_empty() {
                    screens.push(screen);
                } else {
                    unplaced.push(screen);
                }
            }
        }
        // A monitor added after the layout was saved: keep its default spot
        // unless that overlaps, else put it to the right of everything.
        for mut screen in unplaced {
            if screens.iter().any(|s| s.world.overlaps(&screen.world)) {
                if let Some(all) = bounding_box(screens.iter().map(|s| s.world)) {
                    screen.world.x = all.right();
                    screen.world.y = all.y;
                }
            }
            screens.push(screen);
        }
        World { screens }
    }

    #[cfg(test)]
    pub fn placements(&self) -> Vec<Placement> {
        self.screens
            .iter()
            .map(|s| Placement {
                pc: s.pc.clone(),
                monitor: s.monitor.id.clone(),
                x: s.world.x,
                y: s.world.y,
            })
            .collect()
    }

    /// This PC's monitor containing a native point.
    pub fn local_at(&self, p: Point) -> Option<usize> {
        self.screens
            .iter()
            .position(|s| s.local && s.native().contains(p))
    }

    pub fn world_at(&self, p: Point) -> Option<usize> {
        self.screens.iter().position(|s| s.world.contains(p))
    }

    pub fn to_world(&self, screen: usize, native: Point) -> Point {
        let s = &self.screens[screen];
        Point::new(
            native.x - s.native().x + s.world.x,
            native.y - s.native().y + s.world.y,
        )
    }

    pub fn to_native(&self, screen: usize, world: Point) -> Point {
        let s = &self.screens[screen];
        Point::new(
            world.x - s.world.x + s.native().x,
            world.y - s.world.y + s.native().y,
        )
    }

    /// True when a screen of the other PC touches a screen of this PC, i.e.
    /// the pointer can actually get there.
    pub fn remote_reachable(&self) -> bool {
        self.screens.iter().filter(|s| !s.local).any(|r| {
            self.screens
                .iter()
                .filter(|s| s.local)
                .any(|l| touching(&l.world, &r.world))
        })
    }
}

fn bounding_box(rects: impl Iterator<Item = Rect>) -> Option<Rect> {
    rects.reduce(|a, b| {
        let x = a.x.min(b.x);
        let y = a.y.min(b.y);
        Rect::new(
            x,
            y,
            a.right().max(b.right()) - x,
            a.bottom().max(b.bottom()) - y,
        )
    })
}

/// Edges touch with some overlap (the pointer can cross between them).
fn touching(a: &Rect, b: &Rect) -> bool {
    let vertical_overlap = a.y < b.bottom() && b.y < a.bottom();
    let horizontal_overlap = a.x < b.right() && b.x < a.right();
    ((a.right() == b.x || b.right() == a.x) && vertical_overlap)
        || ((a.bottom() == b.y || b.bottom() == a.y) && horizontal_overlap)
}

/// What to do with a mouse move on this PC while it has the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalMove {
    /// Let Windows move the cursor as usual.
    Pass,
    /// Block the move and put the cursor here instead (native coordinates).
    Warp(Point),
    /// Hand the pointer to the other PC, entering `screen` at `at` (world).
    Enter { screen: usize, at: Point },
}

const SIDES: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

/// How the pointer may cross, from the user's settings and this moment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MoveRules {
    /// A mouse button is down. A drag never goes to the other PC; it carries on
    /// to this PC's next screen instead, past the other PC's.
    pub holding: bool,
    /// Don't hand the pointer over near a screen's corners, where the close
    /// button, the Start button and the clock are.
    pub guard_corners: bool,
}

/// Size of a corner that doesn't hand the pointer over, for an edge this long
/// (about a title bar's height).
pub fn corner_size(edge: i32) -> i32 {
    (edge / 36).clamp(16, 60)
}

fn in_corner(native: &Rect, pt: Point, (dx, _): (i32, i32)) -> bool {
    let (along, start, len) = if dx != 0 {
        (pt.y, native.y, native.height)
    } else {
        (pt.x, native.x, native.width)
    };
    let corner = corner_size(len);
    along - start < corner || start + len - 1 - along < corner
}

/// The side of `native` that `pt` went past.
fn side(native: &Rect, pt: Point) -> (i32, i32) {
    if pt.x >= native.right() {
        (1, 0)
    } else if pt.x < native.x {
        (-1, 0)
    } else if pt.y >= native.bottom() {
        (0, 1)
    } else {
        (0, -1)
    }
}

/// Where a drag leaving screen `from` toward `dir` continues: the nearest
/// screen ahead of the same PC (`local` = this PC's), preferring one level
/// with the pointer. That screen and the world point at its near edge; None
/// when that PC has nothing ahead. A drag never changes PCs.
fn drag_past(
    world: &World,
    from: usize,
    target: Point,
    dir: (i32, i32),
    local: bool,
) -> Option<(usize, Point)> {
    let origin = world.screens[from].world;
    let outside = |v: i32, lo: i32, hi: i32| (lo - v).max(v - (hi - 1)).max(0);
    world
        .screens
        .iter()
        .enumerate()
        .filter(|(i, s)| s.local == local && *i != from)
        .filter_map(|(i, s)| {
            let r = s.world;
            let gap = match dir {
                (1, 0) => r.x - origin.right(),
                (-1, 0) => origin.x - r.right(),
                (0, 1) => r.y - origin.bottom(),
                _ => origin.y - r.bottom(),
            };
            let off = if dir.0 != 0 {
                outside(target.y, r.y, r.bottom())
            } else {
                outside(target.x, r.x, r.right())
            };
            (gap >= 0).then_some(((off, gap), i))
        })
        .min_by_key(|(key, _)| *key)
        .map(|(_, i)| {
            let r = world.screens[i].world;
            let entry = match dir {
                (1, 0) => Point::new(r.x, target.y),
                (-1, 0) => Point::new(r.right() - 1, target.y),
                (0, 1) => Point::new(target.x, r.y),
                _ => Point::new(target.x, r.bottom() - 1),
            };
            (i, r.clamp(entry))
        })
}

/// A drag on this PC carried past the other PC's screens, in native pixels.
fn drag_past_here(world: &World, from: usize, target: Point, dir: (i32, i32)) -> Option<Point> {
    drag_past(world, from, target, dir, true).map(|(i, at)| world.to_native(i, at))
}

/// Decide a mouse move from `before` to `pt` (native, this PC). `pt` is where
/// Windows is about to put the cursor, already clamped to the desktop.
pub fn local_move(world: &World, before: Point, pt: Point, rules: MoveRules) -> LocalMove {
    let Some(cur) = world.local_at(before) else {
        return LocalMove::Pass;
    };
    let native = world.screens[cur].native();

    if native.contains(pt) {
        // Pushing against an edge with nothing native beyond it (the edge of
        // Windows' desktop): the arrangement may have a screen there.
        for (dx, dy) in SIDES {
            let on_edge = match (dx, dy) {
                (1, 0) => pt.x == native.right() - 1,
                (-1, 0) => pt.x == native.x,
                (0, 1) => pt.y == native.bottom() - 1,
                _ => pt.y == native.y,
            };
            // Arrived at the edge moving toward it, or pushed while clamped.
            // Sliding along an edge doesn't count.
            let toward = match (dx, dy) {
                (1, 0) => pt.x > before.x,
                (-1, 0) => pt.x < before.x,
                (0, 1) => pt.y > before.y,
                _ => pt.y < before.y,
            };
            if !on_edge || !(toward || pt == before) {
                continue;
            }
            let beyond = Point::new(pt.x + dx, pt.y + dy);
            if world.local_at(beyond).is_some() {
                continue;
            }
            let target = world.to_world(cur, beyond);
            match world.world_at(target) {
                Some(next) if next != cur => {
                    return cross(world, cur, next, target, (dx, dy), pt, rules)
                }
                _ => {}
            }
        }
        return LocalMove::Pass;
    }

    // Leaving the monitor: the arrangement decides where to, not Windows.
    let target = world.to_world(cur, pt);
    match world.world_at(target) {
        Some(next) if world.screens[next].local => {
            let native_target = world.to_native(next, target);
            if native_target == pt {
                LocalMove::Pass
            } else {
                LocalMove::Warp(native_target)
            }
        }
        Some(next) => {
            let dir = side(&native, pt);
            if rules.holding {
                // A drag skips the other PC's screens to this PC's next one.
                LocalMove::Warp(drag_past_here(world, cur, target, dir).unwrap_or(native.clamp(pt)))
            } else if rules.guard_corners && in_corner(&native, native.clamp(pt), dir) {
                LocalMove::Warp(native.clamp(pt))
            } else {
                LocalMove::Enter {
                    screen: next,
                    at: target,
                }
            }
        }
        // Nothing there: stay on this monitor.
        None => LocalMove::Warp(native.clamp(pt)),
    }
}

fn cross(
    world: &World,
    cur: usize,
    next: usize,
    target: Point,
    dir: (i32, i32),
    pt: Point,
    rules: MoveRules,
) -> LocalMove {
    if world.screens[next].local {
        LocalMove::Warp(world.to_native(next, target))
    } else if rules.holding {
        drag_past_here(world, cur, target, dir).map_or(LocalMove::Pass, LocalMove::Warp)
    } else if rules.guard_corners && in_corner(&world.screens[cur].native(), pt, dir) {
        LocalMove::Pass
    } else {
        LocalMove::Enter {
            screen: next,
            at: target,
        }
    }
}

/// One step of the pointer while it is on the other PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteStep {
    /// Still on the other PC: move it to `at` (world) on `screen`.
    Move { screen: usize, at: Point },
    /// Back on this PC, on `screen` at `at` (world).
    Leave { screen: usize, at: Point },
}

/// `rules.holding` = a button is down on the other PC: that drag stays there,
/// carried past this PC's screens to the other PC's next one.
pub fn remote_step(
    world: &World,
    screen: usize,
    at: Point,
    delta: Point,
    rules: MoveRules,
) -> RemoteStep {
    let p = Point::new(at.x + delta.x, at.y + delta.y);
    let current = world.screens[screen].world;
    if current.contains(p) {
        return RemoteStep::Move { screen, at: p };
    }
    let stay = RemoteStep::Move {
        screen,
        at: current.clamp(p),
    };
    match world.world_at(p) {
        Some(next) if world.screens[next].local => {
            let dir = side(&current, p);
            if rules.holding {
                drag_past(world, screen, p, dir, false)
                    .map_or(stay, |(screen, at)| RemoteStep::Move { screen, at })
            } else if rules.guard_corners && in_corner(&current, current.clamp(p), dir) {
                stay
            } else {
                RemoteStep::Leave {
                    screen: next,
                    at: p,
                }
            }
        }
        Some(next) => RemoteStep::Move {
            screen: next,
            at: p,
        },
        None => stay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FREE: MoveRules = MoveRules {
        holding: false,
        guard_corners: false,
    };
    const DRAG: MoveRules = MoveRules {
        holding: true,
        guard_corners: false,
    };
    const GUARDED: MoveRules = MoveRules {
        holding: false,
        guard_corners: true,
    };

    fn monitor(id: &str, number: u32, x: i32, y: i32, w: i32, h: i32) -> Monitor {
        Monitor {
            id: id.into(),
            number,
            name: format!("Display {number}"),
            rect: Rect::new(x, y, w, h),
            primary: x == 0 && y == 0,
        }
    }

    /// The owner's desk: PC1 has A (left) and B (right), side by side in
    /// Windows; PC2's single screen C physically sits between them.
    fn desk() -> (Vec<Monitor>, Vec<Monitor>) {
        (
            vec![
                monitor("A", 1, 0, 0, 1920, 1080),
                monitor("B", 2, 1920, 0, 1920, 1080),
            ],
            vec![monitor("C", 1, 0, 0, 2560, 1440)],
        )
    }

    fn a_c_b(pc1: &[Monitor], pc2: &[Monitor]) -> World {
        let placements = vec![
            Placement {
                pc: "pc1".into(),
                monitor: "A".into(),
                x: 0,
                y: 0,
            },
            Placement {
                pc: "pc2".into(),
                monitor: "C".into(),
                x: 1920,
                y: -180,
            },
            Placement {
                pc: "pc1".into(),
                monitor: "B".into(),
                x: 1920 + 2560,
                y: 0,
            },
        ];
        World::build(
            PcScreens {
                pc: "pc1",
                monitors: pc1,
            },
            PcScreens {
                pc: "pc2",
                monitors: pc2,
            },
            &placements,
        )
    }

    fn index(world: &World, id: &str) -> usize {
        world
            .screens
            .iter()
            .position(|s| s.monitor.id == id)
            .unwrap()
    }

    #[test]
    fn default_puts_the_second_pc_to_the_right_on_both_sides() {
        let (pc1, pc2) = desk();
        let from1 = World::build(
            PcScreens {
                pc: "pc1",
                monitors: &pc1,
            },
            PcScreens {
                pc: "pc2",
                monitors: &pc2,
            },
            &[],
        );
        let from2 = World::build(
            PcScreens {
                pc: "pc2",
                monitors: &pc2,
            },
            PcScreens {
                pc: "pc1",
                monitors: &pc1,
            },
            &[],
        );
        let c1 = &from1.screens[index(&from1, "C")];
        assert_eq!(c1.world, Rect::new(3840, 0, 2560, 1440));
        assert!(!c1.local);
        // The other PC computes the same arrangement.
        let mut a = from1.placements();
        let mut b = from2.placements();
        a.sort_by(|x, y| x.monitor.cmp(&y.monitor));
        b.sort_by(|x, y| x.monitor.cmp(&y.monitor));
        assert_eq!(a, b);
        assert!(from1.remote_reachable());
    }

    #[test]
    fn a_to_c_even_though_windows_would_go_to_b() {
        let (pc1, pc2) = desk();
        let world = a_c_b(&pc1, &pc2);
        // Windows moves the cursor from A's right edge into B…
        let decision = local_move(&world, Point::new(1918, 500), Point::new(1925, 500), FREE);
        // …but the pointer enters C, 5 px in, at the same height on the desk.
        assert_eq!(
            decision,
            LocalMove::Enter {
                screen: index(&world, "C"),
                at: Point::new(1925, 500)
            }
        );
        assert_eq!(
            world.to_native(index(&world, "C"), Point::new(1925, 500)),
            Point::new(5, 680)
        );
    }

    #[test]
    fn c_to_b_when_leaving_the_other_pc_to_the_right() {
        let (pc1, pc2) = desk();
        let world = a_c_b(&pc1, &pc2);
        let c = index(&world, "C");
        let b = index(&world, "B");
        let step = remote_step(
            &world,
            c,
            Point::new(1920 + 2555, 400),
            Point::new(10, 0),
            FREE,
        );
        assert_eq!(
            step,
            RemoteStep::Leave {
                screen: b,
                at: Point::new(4485, 400)
            }
        );
        // B's left edge, in PC1's own coordinates.
        assert_eq!(
            world.to_native(b, Point::new(4485, 400)),
            Point::new(1925, 400)
        );
        // And back left from B goes to C, not to A.
        let decision = local_move(&world, Point::new(1921, 400), Point::new(1915, 400), FREE);
        assert_eq!(
            decision,
            LocalMove::Enter {
                screen: c,
                at: Point::new(4475, 400)
            }
        );
    }

    #[test]
    fn remote_moves_stay_on_the_other_pc_and_clamp_at_open_edges() {
        let (pc1, pc2) = desk();
        let world = a_c_b(&pc1, &pc2);
        let c = index(&world, "C");
        assert_eq!(
            remote_step(&world, c, Point::new(3000, 600), Point::new(40, -30), FREE),
            RemoteStep::Move {
                screen: c,
                at: Point::new(3040, 570)
            }
        );
        // Top of C has nothing above it: the pointer stops at the edge.
        assert_eq!(
            remote_step(&world, c, Point::new(3000, -170), Point::new(0, -50), FREE),
            RemoteStep::Move {
                screen: c,
                at: Point::new(3000, -180)
            }
        );
    }

    #[test]
    fn outer_edge_push_hands_over_when_something_is_there() {
        let (pc1, pc2) = desk();
        // Default arrangement: C to the right of B. B's right edge is the edge
        // of PC1's desktop, so Windows clamps the cursor there.
        let world = World::build(
            PcScreens {
                pc: "pc1",
                monitors: &pc1,
            },
            PcScreens {
                pc: "pc2",
                monitors: &pc2,
            },
            &[],
        );
        let c = index(&world, "C");
        let edge = Point::new(3839, 300);
        assert_eq!(
            local_move(&world, Point::new(3830, 300), edge, FREE),
            LocalMove::Enter {
                screen: c,
                at: Point::new(3840, 300)
            }
        );
        // Pushing while already clamped also counts.
        assert_eq!(
            local_move(&world, edge, edge, FREE),
            LocalMove::Enter {
                screen: c,
                at: Point::new(3840, 300)
            }
        );
        // Sliding down along the edge doesn't.
        assert_eq!(
            local_move(&world, edge, Point::new(3839, 310), FREE),
            LocalMove::Pass
        );
        // The left edge of A has nothing beyond it.
        assert_eq!(
            local_move(&world, Point::new(2, 300), Point::new(0, 300), FREE),
            LocalMove::Pass
        );
    }

    #[test]
    fn drags_and_gaps_keep_the_pointer_home() {
        let (pc1, pc2) = desk();
        let world = a_c_b(&pc1, &pc2);
        // Holding a button (dragging a window): never onto C, the other PC's
        // screen, but on past it to B, this PC's next screen, level with the pointer.
        assert_eq!(
            local_move(&world, Point::new(1918, 500), Point::new(1925, 500), DRAG),
            LocalMove::Warp(Point::new(1920, 500))
        );
        // And back from B's left edge to A's right edge.
        assert_eq!(
            local_move(&world, Point::new(1922, 700), Point::new(1915, 700), DRAG),
            LocalMove::Warp(Point::new(1919, 700))
        );
        // C is taller than A; where C doesn't reach... it does here, so check a
        // layout with a gap: C moved far down, A's right edge leads nowhere at y=100.
        let placements = vec![
            Placement {
                pc: "pc1".into(),
                monitor: "A".into(),
                x: 0,
                y: 0,
            },
            Placement {
                pc: "pc2".into(),
                monitor: "C".into(),
                x: 1920,
                y: 600,
            },
            Placement {
                pc: "pc1".into(),
                monitor: "B".into(),
                x: 1920 + 2560,
                y: 0,
            },
        ];
        let world = World::build(
            PcScreens {
                pc: "pc1",
                monitors: &pc1,
            },
            PcScreens {
                pc: "pc2",
                monitors: &pc2,
            },
            &placements,
        );
        assert_eq!(
            local_move(&world, Point::new(1918, 100), Point::new(1925, 100), FREE),
            LocalMove::Warp(Point::new(1919, 100))
        );
    }

    /// The other PC has two screens with this PC's in the middle (the owner's
    /// desk seen from the PC in the middle): D | M | E, M local.
    fn d_m_e() -> World {
        let middle = vec![monitor("M", 1, 0, 0, 1920, 1080)];
        let sides = vec![
            monitor("D", 1, 0, 0, 1366, 768),
            monitor("E", 2, 1366, 0, 1366, 768),
        ];
        let placements = vec![
            Placement {
                pc: "side".into(),
                monitor: "D".into(),
                x: -1366,
                y: 156,
            },
            Placement {
                pc: "mid".into(),
                monitor: "M".into(),
                x: 0,
                y: 0,
            },
            Placement {
                pc: "side".into(),
                monitor: "E".into(),
                x: 1920,
                y: 156,
            },
        ];
        World::build(
            PcScreens {
                pc: "mid",
                monitors: &middle,
            },
            PcScreens {
                pc: "side",
                monitors: &sides,
            },
            &placements,
        )
    }

    #[test]
    fn a_drag_on_the_other_pc_skips_this_pcs_screen() {
        let world = d_m_e();
        let d = index(&world, "D");
        let e = index(&world, "E");
        // Dragging a tab right on D, toward M (this PC): it goes on to E.
        let holding = MoveRules {
            holding: true,
            guard_corners: false,
        };
        assert_eq!(
            remote_step(&world, d, Point::new(-3, 400), Point::new(8, 0), holding),
            RemoteStep::Move {
                screen: e,
                at: Point::new(1920, 400)
            }
        );
        // And back from E to D.
        assert_eq!(
            remote_step(&world, e, Point::new(1922, 500), Point::new(-5, 0), holding),
            RemoteStep::Move {
                screen: d,
                at: Point::new(-1, 500)
            }
        );
        // Without a button held, the pointer comes home to M as usual.
        assert_eq!(
            remote_step(&world, d, Point::new(-3, 400), Point::new(8, 0), FREE),
            RemoteStep::Leave {
                screen: index(&world, "M"),
                at: Point::new(5, 400)
            }
        );
    }

    #[test]
    fn corners_of_the_other_pc_keep_the_pointer_there() {
        let world = d_m_e();
        let d = index(&world, "D");
        // D's top-right corner (its world top is at y = 156).
        assert_eq!(
            remote_step(&world, d, Point::new(-3, 160), Point::new(8, 0), GUARDED),
            RemoteStep::Move {
                screen: d,
                at: Point::new(-1, 160)
            }
        );
    }

    #[test]
    fn drags_with_nothing_of_this_pc_ahead_stay_put() {
        let (pc1, pc2) = desk();
        // Default: A B | C. Dragging right at B's desktop edge: only C is there.
        let world = World::build(
            PcScreens {
                pc: "pc1",
                monitors: &pc1,
            },
            PcScreens {
                pc: "pc2",
                monitors: &pc2,
            },
            &[],
        );
        let edge = Point::new(3839, 300);
        assert_eq!(local_move(&world, edge, edge, DRAG), LocalMove::Pass);
        // Without the button, the pointer goes over to C as usual.
        assert!(matches!(
            local_move(&world, edge, edge, FREE),
            LocalMove::Enter { .. }
        ));
    }

    #[test]
    fn corners_keep_the_pointer_when_guarded() {
        let (pc1, pc2) = desk();
        let world = a_c_b(&pc1, &pc2);
        // A's top-right corner: the close button of a maximized window.
        let corner = corner_size(1080);
        assert_eq!(corner, 30);
        assert_eq!(
            local_move(&world, Point::new(1915, 10), Point::new(1925, 10), GUARDED),
            LocalMove::Warp(Point::new(1919, 10))
        );
        assert_eq!(
            local_move(
                &world,
                Point::new(1915, 1070),
                Point::new(1925, 1070),
                GUARDED
            ),
            LocalMove::Warp(Point::new(1919, 1070))
        );
        // Away from the corners it crosses; unguarded it crosses anywhere.
        assert!(matches!(
            local_move(
                &world,
                Point::new(1915, 500),
                Point::new(1925, 500),
                GUARDED
            ),
            LocalMove::Enter { .. }
        ));
        assert!(matches!(
            local_move(&world, Point::new(1915, 10), Point::new(1925, 10), FREE),
            LocalMove::Enter { .. }
        ));
    }

    #[test]
    fn rearranged_local_screens_are_followed_with_a_warp() {
        let (pc1, pc2) = desk();
        // The user swapped A and B on the desk (B left of A), C on the far right.
        let placements = vec![
            Placement {
                pc: "pc1".into(),
                monitor: "B".into(),
                x: 0,
                y: 0,
            },
            Placement {
                pc: "pc1".into(),
                monitor: "A".into(),
                x: 1920,
                y: 0,
            },
            Placement {
                pc: "pc2".into(),
                monitor: "C".into(),
                x: 3840,
                y: 0,
            },
        ];
        let world = World::build(
            PcScreens {
                pc: "pc1",
                monitors: &pc1,
            },
            PcScreens {
                pc: "pc2",
                monitors: &pc2,
            },
            &placements,
        );
        // Right from A (Windows would go into B): A is now rightmost of PC1, so C.
        assert_eq!(
            local_move(&world, Point::new(1918, 10), Point::new(1921, 10), FREE),
            LocalMove::Enter {
                screen: index(&world, "C"),
                at: Point::new(3841, 10)
            }
        );
        // Left from A's left edge (desktop edge in Windows) goes to B's right edge.
        assert_eq!(
            local_move(&world, Point::new(3, 10), Point::new(0, 10), FREE),
            LocalMove::Warp(Point::new(3839, 10))
        );
    }

    #[test]
    fn new_monitors_without_a_placement_never_overlap() {
        let (pc1, pc2) = desk();
        let placements = vec![
            Placement {
                pc: "pc1".into(),
                monitor: "A".into(),
                x: 0,
                y: 0,
            },
            Placement {
                pc: "pc2".into(),
                monitor: "C".into(),
                x: 1920,
                y: 0,
            },
        ];
        // B has no placement and its default (1920, 0) is taken by C.
        let world = World::build(
            PcScreens {
                pc: "pc1",
                monitors: &pc1,
            },
            PcScreens {
                pc: "pc2",
                monitors: &pc2,
            },
            &placements,
        );
        let b = &world.screens[index(&world, "B")];
        assert_eq!(b.world, Rect::new(1920 + 2560, 0, 1920, 1080));
    }
}
