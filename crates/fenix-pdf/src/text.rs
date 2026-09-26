//! What's on a page besides its pixels: its characters, each with its
//! box, and its links. Positions are in points from the page's top-left,
//! like search matches. `render.rs` does the pdfium walks that make them.

/// One character and where it is: `[x0, y0, x1, y1]`. Line breaks come as
/// `'\n'` (pdfium puts one where a line ends), with an empty box.
#[derive(Debug, Clone, PartialEq)]
pub struct PageChar {
    pub c: char,
    pub rect: [f32; 4],
}

/// Where a link goes.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkTarget {
    /// A page of this document (from 0), and how far down it in points
    /// from its top when the link says.
    Page { page: u32, y: Option<f32> },
    /// A web or mail address.
    Uri(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PageLink {
    pub rect: [f32; 4],
    pub to: LinkTarget,
}
