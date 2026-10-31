//! What `fenix-pdfium` and the PDF extension say to each other, over its
//! stdin (requests) and stdout (replies). It's a contract with the
//! extension (`fenix.pdf`, in fenix-extensions), which has its own copy
//! of this codec: change one, change both, and bump [`VERSION`].
//!
//! Every message is a frame: its length as a little-endian `u32`, then
//! that many bytes -- a tag byte, then the message's fields in order.
//! Numbers are little-endian (`u8`, `u32`, `u64`, `f32`); a string is a
//! `u32` byte count and UTF-8; a list is a `u32` count and its items.
//! Boxes are `[x0, y0, x1, y1]` in points from the page's top left.
//!
//! The program says [`Reply::Hello`] first. Documents are named by the
//! extension (`doc`), as are views (`view`: a pane showing one) and
//! requests (`id`, which only grow). A render is of part of a page: the
//! page laid out `page_w` x `page_h` pixels, and the rectangle `x, y, w,
//! h` of that, as BGRA -- a strip scrolled into view costs a strip.
//! While a document is open the program watches its file, and says
//! [`Reply::Changed`] when it's written again.

/// The protocol's version, in [`Reply::Hello`].
pub const VERSION: u32 = 1;

/// The most a frame may be: a 4096 x 4096 render and some.
pub const MAX_FRAME: usize = 4096 * 4096 * 4 + 4096;

/// What the extension asks.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// Opens the file at `path` as document `doc`: [`Reply::Opened`], or
    /// [`Reply::OpenFailed`].
    Open { doc: u64, path: String },
    Close { doc: u64 },
    /// Part of a page: [`Reply::Rendered`], or [`Reply::RenderFailed`].
    Render { doc: u64, view: u64, id: u64, page: u32, page_w: u32, page_h: u32, x: u32, y: u32, w: u32, h: u32 },
    /// Drops the renders of `view` still waiting that were asked before
    /// request `before`.
    CancelBefore { doc: u64, view: u64, before: u64 },
    /// Its bookmarks: [`Reply::Outline`].
    Outline { doc: u64 },
    /// A page's characters and their boxes: [`Reply::Text`].
    Text { doc: u64, page: u32 },
    /// A page's links: [`Reply::Links`].
    Links { doc: u64, page: u32 },
    /// Every page searched for `query`, a few at a time: one
    /// [`Reply::Found`] a batch, the last `done`. A newer search of the
    /// document drops what's left of an older one.
    Search { doc: u64, id: u64, query: String, match_case: bool },
}

/// Where a link goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// A page of the document (from 0), and how far down it, in points,
    /// when the link says.
    Page { page: u32, y: Option<f32> },
    /// A web or mail address.
    Uri(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub rect: [f32; 4],
    pub to: Target,
}

/// One place a search found.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub page: u32,
    /// The text around it, on one line.
    pub context: String,
    /// Its boxes, one per line it spans.
    pub rects: Vec<[f32; 4]>,
}

/// One bookmark, flattened: a parent comes just before its children.
#[derive(Debug, Clone, PartialEq)]
pub struct Heading {
    pub title: String,
    pub page: u32,
    pub depth: u32,
}

/// What the program says.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// First, once: the protocol it speaks.
    Hello { version: u32 },
    /// Every page's size in points.
    Opened { doc: u64, pages: Vec<(f32, f32)> },
    /// Why it couldn't: no PDFium (the message says where it looked), or
    /// not a PDF it can read.
    OpenFailed { doc: u64, message: String },
    Rendered { doc: u64, view: u64, id: u64, page: u32, x: u32, y: u32, w: u32, h: u32, bgra: Vec<u8> },
    RenderFailed { doc: u64, id: u64, message: String },
    Outline { doc: u64, headings: Vec<Heading> },
    /// A page's characters in reading order, each with its box; a line
    /// break is `'\n'`, with an empty box.
    Text { doc: u64, page: u32, chars: Vec<(char, [f32; 4])> },
    Links { doc: u64, page: u32, links: Vec<Link> },
    Found { doc: u64, id: u64, done: bool, matches: Vec<Match> },
    /// Its file was written again: open it again to see it.
    Changed { doc: u64 },
}

/// Fields written into a frame's body.
#[derive(Default)]
struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn f32(&mut self, v: f32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn str(&mut self, v: &str) -> &mut Self {
        self.bytes(v.as_bytes())
    }
    fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.u32(v.len() as u32);
        self.0.extend_from_slice(v);
        self
    }
    fn rect(&mut self, r: &[f32; 4]) -> &mut Self {
        for v in r {
            self.f32(*v);
        }
        self
    }
    /// The body, framed: its length first.
    fn frame(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.0.len() + 4);
        out.extend_from_slice(&(self.0.len() as u32).to_le_bytes());
        out.extend(self.0);
        out
    }
}

/// Fields read off a frame's body.
struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        if self.0.len() < n {
            return Err("a message cut short".into());
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn bytes(&mut self) -> Result<Vec<u8>, String> {
        let n = self.u32()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    fn str(&mut self) -> Result<String, String> {
        String::from_utf8(self.bytes()?).map_err(|_| "a string that isn't UTF-8".into())
    }
    fn rect(&mut self) -> Result<[f32; 4], String> {
        Ok([self.f32()?, self.f32()?, self.f32()?, self.f32()?])
    }
    /// A count of items, each at least `least` bytes: more than the body
    /// could hold is a broken message, not an allocation.
    fn count(&mut self, least: usize) -> Result<usize, String> {
        let n = self.u32()? as usize;
        if n.saturating_mul(least.max(1)) > self.0.len() {
            return Err("a list longer than its message".into());
        }
        Ok(n)
    }
}

impl Request {
    /// It as a frame.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::default();
        match self {
            Request::Open { doc, path } => w.u8(1).u64(*doc).str(path),
            Request::Close { doc } => w.u8(2).u64(*doc),
            Request::Render { doc, view, id, page, page_w, page_h, x, y, w: width, h } => w.u8(3).u64(*doc).u64(*view).u64(*id).u32(*page).u32(*page_w).u32(*page_h).u32(*x).u32(*y).u32(*width).u32(*h),
            Request::CancelBefore { doc, view, before } => w.u8(4).u64(*doc).u64(*view).u64(*before),
            Request::Outline { doc } => w.u8(5).u64(*doc),
            Request::Text { doc, page } => w.u8(6).u64(*doc).u32(*page),
            Request::Links { doc, page } => w.u8(7).u64(*doc).u32(*page),
            Request::Search { doc, id, query, match_case } => w.u8(8).u64(*doc).u64(*id).str(query).u8(*match_case as u8),
        };
        w.frame()
    }

    /// A frame's body (its length already taken off) as a request.
    pub fn decode(body: &[u8]) -> Result<Request, String> {
        let mut r = Reader(body);
        Ok(match r.u8()? {
            1 => Request::Open { doc: r.u64()?, path: r.str()? },
            2 => Request::Close { doc: r.u64()? },
            3 => Request::Render { doc: r.u64()?, view: r.u64()?, id: r.u64()?, page: r.u32()?, page_w: r.u32()?, page_h: r.u32()?, x: r.u32()?, y: r.u32()?, w: r.u32()?, h: r.u32()? },
            4 => Request::CancelBefore { doc: r.u64()?, view: r.u64()?, before: r.u64()? },
            5 => Request::Outline { doc: r.u64()? },
            6 => Request::Text { doc: r.u64()?, page: r.u32()? },
            7 => Request::Links { doc: r.u64()?, page: r.u32()? },
            8 => Request::Search { doc: r.u64()?, id: r.u64()?, query: r.str()?, match_case: r.u8()? != 0 },
            tag => return Err(format!("no request {tag}")),
        })
    }
}

impl Reply {
    /// It as a frame.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::default();
        match self {
            Reply::Hello { version } => {
                w.u8(0x80).u32(*version);
            }
            Reply::Opened { doc, pages } => {
                w.u8(0x81).u64(*doc).u32(pages.len() as u32);
                for (pw, ph) in pages {
                    w.f32(*pw).f32(*ph);
                }
            }
            Reply::OpenFailed { doc, message } => {
                w.u8(0x82).u64(*doc).str(message);
            }
            Reply::Rendered { doc, view, id, page, x, y, w: width, h, bgra } => {
                w.u8(0x83).u64(*doc).u64(*view).u64(*id).u32(*page).u32(*x).u32(*y).u32(*width).u32(*h).bytes(bgra);
            }
            Reply::RenderFailed { doc, id, message } => {
                w.u8(0x84).u64(*doc).u64(*id).str(message);
            }
            Reply::Outline { doc, headings } => {
                w.u8(0x85).u64(*doc).u32(headings.len() as u32);
                for h in headings {
                    w.str(&h.title).u32(h.page).u32(h.depth);
                }
            }
            Reply::Text { doc, page, chars } => {
                w.u8(0x86).u64(*doc).u32(*page).u32(chars.len() as u32);
                for (c, rect) in chars {
                    w.u32(*c as u32).rect(rect);
                }
            }
            Reply::Links { doc, page, links } => {
                w.u8(0x87).u64(*doc).u32(*page).u32(links.len() as u32);
                for link in links {
                    w.rect(&link.rect);
                    match &link.to {
                        Target::Page { page, y } => {
                            w.u8(0).u32(*page).u8(y.is_some() as u8).f32(y.unwrap_or(0.0));
                        }
                        Target::Uri(uri) => {
                            w.u8(1).str(uri);
                        }
                    }
                }
            }
            Reply::Found { doc, id, done, matches } => {
                w.u8(0x88).u64(*doc).u64(*id).u8(*done as u8).u32(matches.len() as u32);
                for m in matches {
                    w.u32(m.page).str(&m.context).u32(m.rects.len() as u32);
                    for r in &m.rects {
                        w.rect(r);
                    }
                }
            }
            Reply::Changed { doc } => {
                w.u8(0x89).u64(*doc);
            }
        }
        w.frame()
    }

    /// A frame's body as a reply.
    pub fn decode(body: &[u8]) -> Result<Reply, String> {
        let mut r = Reader(body);
        Ok(match r.u8()? {
            0x80 => Reply::Hello { version: r.u32()? },
            0x81 => {
                let doc = r.u64()?;
                let n = r.count(8)?;
                Reply::Opened { doc, pages: (0..n).map(|_| Ok((r.f32()?, r.f32()?))).collect::<Result<_, String>>()? }
            }
            0x82 => Reply::OpenFailed { doc: r.u64()?, message: r.str()? },
            0x83 => Reply::Rendered { doc: r.u64()?, view: r.u64()?, id: r.u64()?, page: r.u32()?, x: r.u32()?, y: r.u32()?, w: r.u32()?, h: r.u32()?, bgra: r.bytes()? },
            0x84 => Reply::RenderFailed { doc: r.u64()?, id: r.u64()?, message: r.str()? },
            0x85 => {
                let doc = r.u64()?;
                let n = r.count(12)?;
                Reply::Outline { doc, headings: (0..n).map(|_| Ok(Heading { title: r.str()?, page: r.u32()?, depth: r.u32()? })).collect::<Result<_, String>>()? }
            }
            0x86 => {
                let (doc, page) = (r.u64()?, r.u32()?);
                let n = r.count(20)?;
                let chars = (0..n).map(|_| Ok((char::from_u32(r.u32()?).unwrap_or('\u{fffd}'), r.rect()?))).collect::<Result<_, String>>()?;
                Reply::Text { doc, page, chars }
            }
            0x87 => {
                let (doc, page) = (r.u64()?, r.u32()?);
                let n = r.count(17)?;
                let links = (0..n)
                    .map(|_| {
                        let rect = r.rect()?;
                        let to = match r.u8()? {
                            0 => {
                                let page = r.u32()?;
                                let has = r.u8()? != 0;
                                let y = r.f32()?;
                                Target::Page { page, y: has.then_some(y) }
                            }
                            1 => Target::Uri(r.str()?),
                            other => return Err(format!("no link kind {other}")),
                        };
                        Ok(Link { rect, to })
                    })
                    .collect::<Result<_, String>>()?;
                Reply::Links { doc, page, links }
            }
            0x88 => {
                let (doc, id, done) = (r.u64()?, r.u64()?, r.u8()? != 0);
                let n = r.count(12)?;
                let matches = (0..n)
                    .map(|_| {
                        let page = r.u32()?;
                        let context = r.str()?;
                        let k = r.count(16)?;
                        Ok(Match { page, context, rects: (0..k).map(|_| r.rect()).collect::<Result<_, String>>()? })
                    })
                    .collect::<Result<_, String>>()?;
                Reply::Found { doc, id, done, matches }
            }
            0x89 => Reply::Changed { doc: r.u64()? },
            tag => return Err(format!("no reply {tag:#x}")),
        })
    }
}

/// Bytes as they come off a pipe, in pieces, cut into frames' bodies.
#[derive(Default)]
pub struct Frames {
    buf: Vec<u8>,
}

impl Frames {
    /// `bytes` added; then every whole frame's body that's come, in turn.
    /// A frame longer than [`MAX_FRAME`] is an error: the stream's lost.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>, String> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        let mut at = 0;
        while self.buf.len() - at >= 4 {
            let len = u32::from_le_bytes(self.buf[at..at + 4].try_into().unwrap()) as usize;
            if len > MAX_FRAME {
                self.buf.clear();
                return Err(format!("a frame of {len} bytes, more than any message"));
            }
            if self.buf.len() - at - 4 < len {
                break;
            }
            out.push(self.buf[at + 4..at + 4 + len].to_vec());
            at += 4 + len;
        }
        self.buf.drain(..at);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(frame: &[u8]) -> &[u8] {
        let len = u32::from_le_bytes(frame[..4].try_into().unwrap()) as usize;
        assert_eq!(frame.len(), len + 4);
        &frame[4..]
    }

    #[test]
    fn every_request_reads_back_as_written() {
        let all = [
            Request::Open { doc: 7, path: "C:/refs/a b.pdf".into() },
            Request::Close { doc: 7 },
            Request::Render { doc: 7, view: 2, id: 99, page: 3, page_w: 1224, page_h: 1584, x: 0, y: 400, w: 1224, h: 60 },
            Request::CancelBefore { doc: 7, view: 2, before: 99 },
            Request::Outline { doc: 7 },
            Request::Text { doc: 7, page: 1 },
            Request::Links { doc: 7, page: 1 },
            Request::Search { doc: 7, id: 5, query: "Packet Länge".into(), match_case: true },
        ];
        for request in all {
            assert_eq!(Request::decode(body(&request.encode())), Ok(request.clone()), "{request:?}");
        }
        assert!(Request::decode(&[42]).is_err());
        assert!(Request::decode(&[1, 0, 0]).is_err(), "cut short");
    }

    #[test]
    fn every_reply_reads_back_as_written() {
        let all = [
            Reply::Hello { version: VERSION },
            Reply::Opened { doc: 1, pages: vec![(612.0, 792.0), (595.0, 842.0)] },
            Reply::OpenFailed { doc: 1, message: "pdfium library not found".into() },
            Reply::Rendered { doc: 1, view: 2, id: 3, page: 0, x: 4, y: 5, w: 1, h: 2, bgra: vec![1, 2, 3, 255, 4, 5, 6, 255] },
            Reply::RenderFailed { doc: 1, id: 3, message: "no page".into() },
            Reply::Outline { doc: 1, headings: vec![Heading { title: "Chapter 1".into(), page: 0, depth: 0 }, Heading { title: "1.1".into(), page: 2, depth: 1 }] },
            Reply::Text { doc: 1, page: 0, chars: vec![('H', [1.0, 2.0, 3.0, 4.0]), ('\n', [0.0; 4])] },
            Reply::Links { doc: 1, page: 0, links: vec![Link { rect: [1.0, 2.0, 3.0, 4.0], to: Target::Page { page: 1, y: Some(10.5) } }, Link { rect: [0.0; 4], to: Target::Page { page: 0, y: None } }, Link { rect: [0.0; 4], to: Target::Uri("https://example.com/".into()) }] },
            Reply::Found { doc: 1, id: 8, done: true, matches: vec![Match { page: 3, context: "a packet here".into(), rects: vec![[1.0, 2.0, 3.0, 4.0]] }] },
            Reply::Changed { doc: 1 },
        ];
        for reply in all {
            assert_eq!(Reply::decode(body(&reply.encode())), Ok(reply.clone()), "{reply:?}");
        }
    }

    #[test]
    fn frames_come_whole_however_the_bytes_are_cut() {
        let stream: Vec<u8> = [Reply::Hello { version: 1 }.encode(), Reply::Changed { doc: 9 }.encode()].concat();
        let mut frames = Frames::default();
        let mut got = Vec::new();
        for b in &stream {
            got.extend(frames.push(&[*b]).unwrap());
        }
        assert_eq!(got.iter().map(|b| Reply::decode(b).unwrap()).collect::<Vec<_>>(), vec![Reply::Hello { version: 1 }, Reply::Changed { doc: 9 }]);
        assert!(Frames::default().push(&u32::MAX.to_le_bytes()).is_err(), "a length no message has");
    }

    #[test]
    fn a_count_bigger_than_its_message_is_refused_not_allocated() {
        let mut w = Writer::default();
        w.u8(0x81).u64(1).u32(u32::MAX);
        assert!(Reply::decode(body(&w.frame())).is_err());
    }
}
