//! Window tree: binary splits over the terminal area. Each leaf shows one
//! buffer with its own point and scroll position (window-point, like Emacs).

use crate::buffer::BufferId;

pub type WindowId = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl Rect {
    /// Rows available for buffer text (reserving the mode line).
    pub fn text_height(&self) -> usize {
        self.h.saturating_sub(1) as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDir {
    /// C-x 2: children stacked above/below.
    Below,
    /// C-x 3: children side by side.
    Right,
}

#[derive(Debug)]
pub struct Window {
    pub id: WindowId,
    pub buffer: BufferId,
    pub point: usize,
    /// First visible buffer line.
    pub top_line: usize,
}

#[derive(Debug)]
pub enum Node {
    Leaf(Window),
    Split {
        dir: SplitDir,
        first: Box<Node>,
        second: Box<Node>,
    },
}

#[derive(Debug)]
pub struct WindowTree {
    pub root: Node,
    pub selected: WindowId,
    next_id: WindowId,
}

impl WindowTree {
    pub fn new(buffer: BufferId) -> Self {
        WindowTree {
            root: Node::Leaf(Window { id: 0, buffer, point: 0, top_line: 0 }),
            selected: 0,
            next_id: 1,
        }
    }

    pub fn selected_mut(&mut self) -> &mut Window {
        let id = self.selected;
        self.find_mut(id).expect("selected window exists")
    }

    pub fn selected_ref(&self) -> &Window {
        let id = self.selected;
        self.find(id).expect("selected window exists")
    }

    pub fn find(&self, id: WindowId) -> Option<&Window> {
        fn walk(node: &Node, id: WindowId) -> Option<&Window> {
            match node {
                Node::Leaf(w) => (w.id == id).then_some(w),
                Node::Split { first, second, .. } => {
                    walk(first, id).or_else(|| walk(second, id))
                }
            }
        }
        walk(&self.root, id)
    }

    pub fn find_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        fn walk(node: &mut Node, id: WindowId) -> Option<&mut Window> {
            match node {
                Node::Leaf(w) => (w.id == id).then_some(w),
                Node::Split { first, second, .. } => {
                    walk(first, id).or_else(|| walk(second, id))
                }
            }
        }
        walk(&mut self.root, id)
    }

    /// Leaf windows in depth-first order (the C-x o cycle order).
    pub fn leaves(&self) -> Vec<WindowId> {
        fn walk(node: &Node, out: &mut Vec<WindowId>) {
            match node {
                Node::Leaf(w) => out.push(w.id),
                Node::Split { first, second, .. } => {
                    walk(first, out);
                    walk(second, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &mut out);
        out
    }

    pub fn windows_mut(&mut self) -> Vec<&mut Window> {
        fn walk<'a>(node: &'a mut Node, out: &mut Vec<&'a mut Window>) {
            match node {
                Node::Leaf(w) => out.push(w),
                Node::Split { first, second, .. } => {
                    walk(first, out);
                    walk(second, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&mut self.root, &mut out);
        out
    }

    /// Split the selected window; the new window shows the same buffer and
    /// becomes the second child. Selection stays in the original.
    pub fn split(&mut self, dir: SplitDir) {
        let new_id = self.next_id;
        self.next_id += 1;
        let sel = self.selected;
        fn walk(node: &mut Node, sel: WindowId, new_id: WindowId, dir: SplitDir) -> bool {
            match node {
                Node::Leaf(w) if w.id == sel => {
                    let twin = Window {
                        id: new_id,
                        buffer: w.buffer,
                        point: w.point,
                        top_line: w.top_line,
                    };
                    let old = std::mem::replace(
                        node,
                        Node::Leaf(Window { id: 0, buffer: 0, point: 0, top_line: 0 }),
                    );
                    *node = Node::Split {
                        dir,
                        first: Box::new(old),
                        second: Box::new(Node::Leaf(twin)),
                    };
                    true
                }
                Node::Leaf(_) => false,
                Node::Split { first, second, .. } => {
                    walk(first, sel, new_id, dir) || walk(second, sel, new_id, dir)
                }
            }
        }
        walk(&mut self.root, sel, new_id, dir);
    }

    /// Delete window `id`; its sibling absorbs the space. No-op on the last
    /// window. Returns false if it was the only window.
    pub fn delete(&mut self, id: WindowId) -> bool {
        if matches!(self.root, Node::Leaf(_)) {
            return false;
        }
        fn walk(node: &mut Node, id: WindowId) -> bool {
            if let Node::Split { first, second, .. } = node {
                let hit = |child: &Node| matches!(child, Node::Leaf(w) if w.id == id);
                if hit(first) {
                    let keep = std::mem::replace(
                        second.as_mut(),
                        Node::Leaf(Window { id: 0, buffer: 0, point: 0, top_line: 0 }),
                    );
                    *node = keep;
                    return true;
                }
                if hit(second) {
                    let keep = std::mem::replace(
                        first.as_mut(),
                        Node::Leaf(Window { id: 0, buffer: 0, point: 0, top_line: 0 }),
                    );
                    *node = keep;
                    return true;
                }
                return walk(first, id) || walk(second, id);
            }
            false
        }
        let deleted = walk(&mut self.root, id);
        if deleted && self.selected == id {
            self.selected = self.leaves()[0];
        }
        deleted
    }

    /// Keep only the selected window (C-x 1).
    pub fn delete_others(&mut self) {
        let sel = self.selected;
        fn take(node: &mut Node, sel: WindowId) -> Option<Node> {
            match node {
                Node::Leaf(w) if w.id == sel => Some(std::mem::replace(
                    node,
                    Node::Leaf(Window { id: 0, buffer: 0, point: 0, top_line: 0 }),
                )),
                Node::Leaf(_) => None,
                Node::Split { first, second, .. } => {
                    take(first, sel).or_else(|| take(second, sel))
                }
            }
        }
        if let Some(keep) = take(&mut self.root, sel) {
            self.root = keep;
        }
    }

    pub fn select_next(&mut self) {
        let leaves = self.leaves();
        if let Some(pos) = leaves.iter().position(|&id| id == self.selected) {
            self.selected = leaves[(pos + 1) % leaves.len()];
        }
    }

    /// Compute leaf rectangles for the given text area.
    pub fn layout(&self, area: Rect) -> Vec<(WindowId, Rect)> {
        fn walk(node: &Node, rect: Rect, out: &mut Vec<(WindowId, Rect)>) {
            match node {
                Node::Leaf(w) => out.push((w.id, rect)),
                Node::Split { dir: SplitDir::Below, first, second } => {
                    let top_h = rect.h / 2;
                    walk(first, Rect { h: top_h, ..rect }, out);
                    walk(
                        second,
                        Rect { y: rect.y + top_h, h: rect.h - top_h, ..rect },
                        out,
                    );
                }
                Node::Split { dir: SplitDir::Right, first, second } => {
                    // Reserve one column for the vertical divider.
                    let left_w = rect.w / 2;
                    walk(first, Rect { w: left_w.saturating_sub(1), ..rect }, out);
                    walk(
                        second,
                        Rect { x: rect.x + left_w, w: rect.w - left_w, ..rect },
                        out,
                    );
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.root, area, &mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_delete_cycle() {
        let mut t = WindowTree::new(0);
        t.split(SplitDir::Below);
        t.split(SplitDir::Right);
        assert_eq!(t.leaves().len(), 3);
        t.select_next();
        let second = t.selected;
        assert!(t.delete(second));
        assert_eq!(t.leaves().len(), 2);
        t.delete_others();
        assert_eq!(t.leaves().len(), 1);
        assert!(!t.delete(t.selected));
    }

    #[test]
    fn layout_partitions_area() {
        let mut t = WindowTree::new(0);
        t.split(SplitDir::Below);
        let area = Rect { x: 0, y: 0, w: 80, h: 24 };
        let rects = t.layout(area);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0].1.h + rects[1].1.h, 24);
    }
}
