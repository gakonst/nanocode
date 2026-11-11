// `LS_COLORS` styles for fd's colored output (D1138): what fd uses of lscolors 0.21.0
// (https://github.com/sharkdp/lscolors: `LsColors`'s builder, indicators and suffix
// map, `Style::from_ansi_sequence`) and of nu-ansi-term 0.50.3
// (https://github.com/nushell/nu-ansi-term: `Style`'s prefix and suffix), adapted, and
// fd's default colors from src/main.rs of fd (https://github.com/sharkdp/fd, commit
// 193d0df):
// Copyright (c) David Peter (lscolors). Licensed under the MIT License (see NOTICE).
// Copyright (c) 2014 Benjamin Sago, Copyright (c) 2021-2022 The Nushell Project
// Developers (nu-ansi-term). Licensed under the MIT License (see NOTICE).
// Copyright (c) 2017-present The fd developers. Licensed under the MIT License (see NOTICE).

//! Only what fd prints: a style per entry (its type, else its suffix), rendered as
//! nu-ansi-term renders it (attributes, then background, then foreground; lscolors'
//! bright colors as 256-color codes). No aho-corasick: the suffix map is searched
//! directly, with the same precedence (the last definition wins; ASCII case is ignored
//! unless two spellings of a suffix have different styles).

use std::collections::HashMap;
use std::fmt::Write as _;

use psh_engine::host::FileKind;

// vivid --color-mode 8-bit generate molokai
const DEFAULT_LS_COLORS: &str = "
ow=0:or=0;38;5;16;48;5;203:no=0:ex=1;38;5;203:cd=0;38;5;203;48;5;236:mi=0;38;5;16;48;5;203:*~=0;38;5;243:st=0:pi=0;38;5;16;48;5;81:fi=0:di=0;38;5;81:so=0;38;5;16;48;5;203:bd=0;38;5;81;48;5;236:tw=0:ln=0;38;5;203:*.m=0;38;5;48:*.o=0;38;5;243:*.z=4;38;5;203:*.a=1;38;5;203:*.r=0;38;5;48:*.c=0;38;5;48:*.d=0;38;5;48:*.t=0;38;5;48:*.h=0;38;5;48:*.p=0;38;5;48:*.cc=0;38;5;48:*.ll=0;38;5;48:*.jl=0;38;5;48:*css=0;38;5;48:*.md=0;38;5;185:*.gz=4;38;5;203:*.nb=0;38;5;48:*.mn=0;38;5;48:*.go=0;38;5;48:*.xz=4;38;5;203:*.so=1;38;5;203:*.rb=0;38;5;48:*.pm=0;38;5;48:*.bc=0;38;5;243:*.py=0;38;5;48:*.as=0;38;5;48:*.pl=0;38;5;48:*.rs=0;38;5;48:*.sh=0;38;5;48:*.7z=4;38;5;203:*.ps=0;38;5;186:*.cs=0;38;5;48:*.el=0;38;5;48:*.rm=0;38;5;208:*.hs=0;38;5;48:*.td=0;38;5;48:*.ui=0;38;5;149:*.ex=0;38;5;48:*.js=0;38;5;48:*.cp=0;38;5;48:*.cr=0;38;5;48:*.la=0;38;5;243:*.kt=0;38;5;48:*.ml=0;38;5;48:*.vb=0;38;5;48:*.gv=0;38;5;48:*.lo=0;38;5;243:*.hi=0;38;5;243:*.ts=0;38;5;48:*.ko=1;38;5;203:*.hh=0;38;5;48:*.pp=0;38;5;48:*.di=0;38;5;48:*.bz=4;38;5;203:*.fs=0;38;5;48:*.png=0;38;5;208:*.zsh=0;38;5;48:*.mpg=0;38;5;208:*.pid=0;38;5;243:*.xmp=0;38;5;149:*.iso=4;38;5;203:*.m4v=0;38;5;208:*.dot=0;38;5;48:*.ods=0;38;5;186:*.inc=0;38;5;48:*.sxw=0;38;5;186:*.aif=0;38;5;208:*.git=0;38;5;243:*.gvy=0;38;5;48:*.tbz=4;38;5;203:*.log=0;38;5;243:*.txt=0;38;5;185:*.ico=0;38;5;208:*.csx=0;38;5;48:*.vob=0;38;5;208:*.pgm=0;38;5;208:*.pps=0;38;5;186:*.ics=0;38;5;186:*.img=4;38;5;203:*.fon=0;38;5;208:*.hpp=0;38;5;48:*.bsh=0;38;5;48:*.sql=0;38;5;48:*TODO=1:*.php=0;38;5;48:*.pkg=4;38;5;203:*.ps1=0;38;5;48:*.csv=0;38;5;185:*.ilg=0;38;5;243:*.ini=0;38;5;149:*.pyc=0;38;5;243:*.psd=0;38;5;208:*.htc=0;38;5;48:*.swp=0;38;5;243:*.mli=0;38;5;48:*hgrc=0;38;5;149:*.bst=0;38;5;149:*.ipp=0;38;5;48:*.fsi=0;38;5;48:*.tcl=0;38;5;48:*.exs=0;38;5;48:*.out=0;38;5;243:*.jar=4;38;5;203:*.xls=0;38;5;186:*.ppm=0;38;5;208:*.apk=4;38;5;203:*.aux=0;38;5;243:*.rpm=4;38;5;203:*.dll=1;38;5;203:*.eps=0;38;5;208:*.exe=1;38;5;203:*.doc=0;38;5;186:*.wma=0;38;5;208:*.deb=4;38;5;203:*.pod=0;38;5;48:*.ind=0;38;5;243:*.nix=0;38;5;149:*.lua=0;38;5;48:*.epp=0;38;5;48:*.dpr=0;38;5;48:*.htm=0;38;5;185:*.ogg=0;38;5;208:*.bin=4;38;5;203:*.otf=0;38;5;208:*.yml=0;38;5;149:*.pro=0;38;5;149:*.cxx=0;38;5;48:*.tex=0;38;5;48:*.fnt=0;38;5;208:*.erl=0;38;5;48:*.sty=0;38;5;243:*.bag=4;38;5;203:*.rst=0;38;5;185:*.pdf=0;38;5;186:*.pbm=0;38;5;208:*.xcf=0;38;5;208:*.clj=0;38;5;48:*.gif=0;38;5;208:*.rar=4;38;5;203:*.elm=0;38;5;48:*.bib=0;38;5;149:*.tsx=0;38;5;48:*.dmg=4;38;5;203:*.tmp=0;38;5;243:*.bcf=0;38;5;243:*.mkv=0;38;5;208:*.svg=0;38;5;208:*.cpp=0;38;5;48:*.vim=0;38;5;48:*.bmp=0;38;5;208:*.ltx=0;38;5;48:*.fls=0;38;5;243:*.flv=0;38;5;208:*.wav=0;38;5;208:*.m4a=0;38;5;208:*.mid=0;38;5;208:*.hxx=0;38;5;48:*.pas=0;38;5;48:*.wmv=0;38;5;208:*.tif=0;38;5;208:*.kex=0;38;5;186:*.mp4=0;38;5;208:*.bak=0;38;5;243:*.xlr=0;38;5;186:*.dox=0;38;5;149:*.swf=0;38;5;208:*.tar=4;38;5;203:*.tgz=4;38;5;203:*.cfg=0;38;5;149:*.xml=0;
38;5;185:*.jpg=0;38;5;208:*.mir=0;38;5;48:*.sxi=0;38;5;186:*.bz2=4;38;5;203:*.odt=0;38;5;186:*.mov=0;38;5;208:*.toc=0;38;5;243:*.bat=1;38;5;203:*.asa=0;38;5;48:*.awk=0;38;5;48:*.sbt=0;38;5;48:*.vcd=4;38;5;203:*.kts=0;38;5;48:*.arj=4;38;5;203:*.blg=0;38;5;243:*.c++=0;38;5;48:*.odp=0;38;5;186:*.bbl=0;38;5;243:*.idx=0;38;5;243:*.com=1;38;5;203:*.mp3=0;38;5;208:*.avi=0;38;5;208:*.def=0;38;5;48:*.cgi=0;38;5;48:*.zip=4;38;5;203:*.ttf=0;38;5;208:*.ppt=0;38;5;186:*.tml=0;38;5;149:*.fsx=0;38;5;48:*.h++=0;38;5;48:*.rtf=0;38;5;186:*.inl=0;38;5;48:*.yaml=0;38;5;149:*.html=0;38;5;185:*.mpeg=0;38;5;208:*.java=0;38;5;48:*.hgrc=0;38;5;149:*.orig=0;38;5;243:*.conf=0;38;5;149:*.dart=0;38;5;48:*.psm1=0;38;5;48:*.rlib=0;38;5;243:*.fish=0;38;5;48:*.bash=0;38;5;48:*.make=0;38;5;149:*.docx=0;38;5;186:*.json=0;38;5;149:*.psd1=0;38;5;48:*.lisp=0;38;5;48:*.tbz2=4;38;5;203:*.diff=0;38;5;48:*.epub=0;38;5;186:*.xlsx=0;38;5;186:*.pptx=0;38;5;186:*.toml=0;38;5;149:*.h264=0;38;5;208:*.purs=0;38;5;48:*.flac=0;38;5;208:*.tiff=0;38;5;208:*.jpeg=0;38;5;208:*.lock=0;38;5;243:*.less=0;38;5;48:*.dyn_o=0;38;5;243:*.scala=0;38;5;48:*.mdown=0;38;5;185:*.shtml=0;38;5;185:*.class=0;38;5;243:*.cache=0;38;5;243:*.cmake=0;38;5;149:*passwd=0;38;5;149:*.swift=0;38;5;48:*shadow=0;38;5;149:*.xhtml=0;38;5;185:*.patch=0;38;5;48:*.cabal=0;38;5;48:*README=0;38;5;16;48;5;186:*.toast=4;38;5;203:*.ipynb=0;38;5;48:*COPYING=0;38;5;249:*.gradle=0;38;5;48:*.matlab=0;38;5;48:*.config=0;38;5;149:*LICENSE=0;38;5;249:*.dyn_hi=0;38;5;243:*.flake8=0;38;5;149:*.groovy=0;38;5;48:*INSTALL=0;38;5;16;48;5;186:*TODO.md=1:*.ignore=0;38;5;149:*Doxyfile=0;38;5;149:*TODO.txt=1:*setup.py=0;38;5;149:*Makefile=0;38;5;149:*.gemspec=0;38;5;149:*.desktop=0;38;5;149:*.rgignore=0;38;5;149:*.markdown=0;38;5;185:*COPYRIGHT=0;38;5;249:*configure=0;38;5;149:*.DS_Store=0;38;5;243:*.kdevelop=0;38;5;149:*.fdignore=0;38;5;149:*README.md=0;38;5;16;48;5;186:*.cmake.in=0;38;5;149:*SConscript=0;38;5;149:*CODEOWNERS=0;38;5;149:*.localized=0;38;5;243:*.gitignore=0;38;5;149:*Dockerfile=0;38;5;149:*.gitconfig=0;38;5;149:*INSTALL.md=0;38;5;16;48;5;186:*README.txt=0;38;5;16;48;5;186:*SConstruct=0;38;5;149:*.scons_opt=0;38;5;243:*.travis.yml=0;38;5;186:*.gitmodules=0;38;5;149:*.synctex.gz=0;38;5;243:*LICENSE-MIT=0;38;5;249:*MANIFEST.in=0;38;5;149:*Makefile.in=0;38;5;243:*Makefile.am=0;38;5;149:*INSTALL.txt=0;38;5;16;48;5;186:*configure.ac=0;38;5;149:*.applescript=0;38;5;48:*appveyor.yml=0;38;5;186:*.fdb_latexmk=0;38;5;243:*CONTRIBUTORS=0;38;5;16;48;5;186:*.clang-format=0;38;5;149:*LICENSE-APACHE=0;38;5;249:*CMakeLists.txt=0;38;5;149:*CMakeCache.txt=0;38;5;243:*.gitattributes=0;38;5;149:*CONTRIBUTORS.md=0;38;5;16;48;5;186:*.sconsign.dblite=0;38;5;243:*requirements.txt=0;38;5;149:*CONTRIBUTORS.txt=0;38;5;16;48;5;186:*package-lock.json=0;38;5;243:*.CFUserTextEncoding=0;38;5;243
";

/// lscolors' defaults, under every `LS_COLORS`.
const LS_COLORS_DEFAULT: &str = "rs=0:lc=\x1b[:rc=m:cl=\x1b[K:ex=01;32:sg=30;43:su=37;41:di=01;34:st=37;44:ow=34;42:tw=30;42:ln=01;36:bd=01;33:cd=01;33:do=01;35:pi=33:so=01;35:";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Indicator {
    Normal,
    RegularFile,
    Directory,
    SymbolicLink,
    Fifo,
    Socket,
    Door,
    BlockDevice,
    CharacterDevice,
    OrphanedSymbolicLink,
    Setuid,
    Setgid,
    Sticky,
    OtherWritable,
    StickyAndOtherWritable,
    ExecutableFile,
    MissingFile,
    Capabilities,
    MultipleHardLinks,
    LeftCode,
    RightCode,
    EndCode,
    Reset,
    ClearLine,
}

impl Indicator {
    pub fn from(indicator: &str) -> Option<Indicator> {
        match indicator {
            "no" => Some(Indicator::Normal),
            "fi" => Some(Indicator::RegularFile),
            "di" => Some(Indicator::Directory),
            "ln" => Some(Indicator::SymbolicLink),
            "pi" => Some(Indicator::Fifo),
            "so" => Some(Indicator::Socket),
            "do" => Some(Indicator::Door),
            "bd" => Some(Indicator::BlockDevice),
            "cd" => Some(Indicator::CharacterDevice),
            "or" => Some(Indicator::OrphanedSymbolicLink),
            "su" => Some(Indicator::Setuid),
            "sg" => Some(Indicator::Setgid),
            "st" => Some(Indicator::Sticky),
            "ow" => Some(Indicator::OtherWritable),
            "tw" => Some(Indicator::StickyAndOtherWritable),
            "ex" => Some(Indicator::ExecutableFile),
            "mi" => Some(Indicator::MissingFile),
            "ca" => Some(Indicator::Capabilities),
            "mh" => Some(Indicator::MultipleHardLinks),
            "lc" => Some(Indicator::LeftCode),
            "rc" => Some(Indicator::RightCode),
            "ec" => Some(Indicator::EndCode),
            "rs" => Some(Indicator::Reset),
            "cl" => Some(Indicator::ClearLine),
            _ => None,
        }
    }
}

/// A color as nu-ansi-term writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    /// The eight basic colors, 0 (black) to 7 (white).
    Basic(u8),
    Fixed(u8),
    Rgb(u8, u8, u8),
}

impl Color {
    fn write(self, out: &mut String, base: u8) {
        let _ = match self {
            Color::Basic(n) => write!(out, "{}", base + n),
            Color::Fixed(n) => write!(out, "{};5;{n}", base + 8),
            Color::Rgb(r, g, b) => write!(out, "{};2;{r};{g};{b}", base + 8),
        };
    }
}

/// nu-ansi-term's `Style`, as lscolors' `to_nu_ansi_term_style` makes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub foreground: Option<Color>,
    pub background: Option<Color>,
    pub bold: bool,
    pub dimmed: bool,
    pub italic: bool,
    pub underline: bool,
    pub blink: bool,
    pub reverse: bool,
    pub hidden: bool,
    pub strikethrough: bool,
}

impl Style {
    /// Parse ANSI escape sequences like `38;2;255;0;100;1;4` (pink, bold, underlined).
    pub fn from_ansi_sequence(code: &str) -> Option<Style> {
        if code.is_empty() || code == "0" || code == "00" {
            return None;
        }
        let mut parts = code
            .split(';')
            .map(|c| c.parse::<u8>().ok())
            .collect::<Option<std::collections::VecDeque<u8>>>()?;
        let mut s = Style::default();
        // lscolors keeps slow and rapid blink apart; nu-ansi-term has one blink.
        let (mut slow, mut rapid) = (false, false);
        // A 256-color or RGB color after 38/48/58.
        fn extended(parts: &mut std::collections::VecDeque<u8>) -> Option<Color> {
            match (parts.pop_front(), parts.pop_front()) {
                (Some(5), Some(c)) => Some(Color::Fixed(c)),
                (Some(2), Some(r)) => match (parts.pop_front(), parts.pop_front()) {
                    (Some(g), Some(b)) => Some(Color::Rgb(r, g, b)),
                    _ => None,
                },
                _ => None,
            }
        }
        // lscolors' bright colors become nu-ansi-term's `Fixed(8..=15)`.
        let bright = |n: u8| Color::Fixed(8 + n);
        loop {
            match parts.pop_front() {
                Some(0) => {
                    let (fg, bg) = (s.foreground, s.background);
                    s = Style {
                        foreground: fg,
                        background: bg,
                        ..Style::default()
                    };
                    (slow, rapid) = (false, false);
                }
                Some(1) => s.bold = true,
                Some(2) => s.dimmed = true,
                Some(3) => s.italic = true,
                Some(4) => s.underline = true,
                Some(5) => slow = true,
                Some(6) => rapid = true,
                Some(7) => s.reverse = true,
                Some(8) => s.hidden = true,
                Some(9) => s.strikethrough = true,
                Some(22) => {
                    s.bold = false;
                    s.dimmed = false;
                }
                Some(23) => s.italic = false,
                Some(24) => s.underline = false,
                Some(25) => (slow, rapid) = (false, false),
                Some(27) => s.reverse = false,
                Some(28) => s.hidden = false,
                Some(29) => s.strikethrough = false,
                Some(n @ 30..=37) => s.foreground = Some(Color::Basic(n - 30)),
                Some(38) => match extended(&mut parts) {
                    Some(c) => s.foreground = Some(c),
                    None => break,
                },
                Some(39) => s.foreground = None,
                Some(n @ 40..=47) => s.background = Some(Color::Basic(n - 40)),
                Some(48) => match extended(&mut parts) {
                    Some(c) => s.background = Some(c),
                    None => break,
                },
                Some(49) => s.background = None,
                // The underline color: parsed, but not rendered by nu-ansi-term.
                Some(58) => {
                    if extended(&mut parts).is_none() {
                        break;
                    }
                }
                Some(n @ 90..=97) => s.foreground = Some(bright(n - 90)),
                Some(n @ 100..=107) => s.background = Some(bright(n - 100)),
                Some(_) => continue,
                None => break,
            }
        }
        s.blink = slow || rapid;
        Some(s)
    }

    fn is_plain(&self) -> bool {
        *self == Style::default()
    }

    /// The codes before a piece of text.
    pub fn prefix(&self) -> String {
        if self.is_plain() {
            return String::new();
        }
        let mut codes: Vec<&str> = Vec::new();
        for (on, code) in [
            (self.bold, "1"),
            (self.dimmed, "2"),
            (self.italic, "3"),
            (self.underline, "4"),
            (self.blink, "5"),
            (self.reverse, "7"),
            (self.hidden, "8"),
            (self.strikethrough, "9"),
        ] {
            if on {
                codes.push(code);
            }
        }
        let mut out = String::from("\x1b[");
        out.push_str(&codes.join(";"));
        let mut written = !codes.is_empty();
        for (color, base) in [(self.background, 40), (self.foreground, 30)] {
            if let Some(c) = color {
                if written {
                    out.push(';');
                }
                written = true;
                c.write(&mut out, base);
            }
        }
        out.push('m');
        out
    }

    /// The codes after a piece of text.
    pub fn suffix(&self) -> &'static str {
        if self.is_plain() { "" } else { "\x1b[0m" }
    }

    /// `text` painted.
    pub fn paint(&self, text: &str) -> String {
        format!("{}{text}{}", self.prefix(), self.suffix())
    }
}

/// What lscolors' `Colorable` asks of an entry.
pub struct Facts<'a> {
    pub name: &'a [u8],
    /// The entry's type, if known.
    pub kind: Option<FileKind>,
    /// Its permission bits, if known.
    pub mode: Option<u32>,
    /// For a symbolic link: whether its target exists.
    pub target_exists: bool,
}

#[derive(Debug, Clone)]
pub struct LsColors {
    indicator_mapping: HashMap<Indicator, Style>,
    /// Whether Indicator::RegularFile falls back to Indicator::Normal
    file_normal_fallback: bool,
    /// Suffixes in definition order.
    suffixes: Vec<(Vec<u8>, Option<Style>)>,
    /// The ASCII-lowercased suffixes matched case-sensitively.
    case_sensitive: Vec<Vec<u8>>,
}

impl LsColors {
    /// The colors fd uses: `LS_COLORS` if set, else fd's defaults, on top of lscolors'
    /// defaults.
    pub fn for_fd(ls_colors: Option<&str>) -> LsColors {
        LsColors::from_string(ls_colors.unwrap_or(DEFAULT_LS_COLORS))
    }

    pub fn from_string(input: &str) -> LsColors {
        let mut c = LsColors {
            indicator_mapping: HashMap::new(),
            file_normal_fallback: true,
            suffixes: Vec::new(),
            case_sensitive: Vec::new(),
        };
        c.add_from_string(LS_COLORS_DEFAULT);
        c.add_from_string(input);
        // A suffix is case-sensitive when its latest definition and the latest
        // definition of any spelling of it differ in style.
        let mut latest_exact: HashMap<&[u8], usize> = HashMap::new();
        let mut latest_folded: HashMap<Vec<u8>, usize> = HashMap::new();
        for (i, (s, _)) in c.suffixes.iter().enumerate() {
            latest_exact.insert(s, i);
            latest_folded.insert(s.to_ascii_lowercase(), i);
        }
        let mut cs = Vec::new();
        for &i in latest_exact.values() {
            let folded = c.suffixes[i].0.to_ascii_lowercase();
            let j = latest_folded[&folded];
            if c.suffixes[i].1 != c.suffixes[j].1 && !cs.contains(&folded) {
                cs.push(folded);
            }
        }
        c.case_sensitive = cs;
        c
    }

    fn add_from_string(&mut self, input: &str) {
        for entry in input.split(':') {
            let parts: Vec<_> = entry.split('=').collect();
            if let Some([entry, ansi_style]) = parts.get(0..2) {
                let style = Style::from_ansi_sequence(ansi_style);
                if let Some(suffix) = entry.strip_prefix('*') {
                    self.suffixes.push((suffix.as_bytes().to_vec(), style));
                } else if let Some(indicator) = Indicator::from(entry) {
                    if let Some(style) = style {
                        self.indicator_mapping.insert(indicator, style);
                    } else {
                        self.indicator_mapping.remove(&indicator);
                        if indicator == Indicator::RegularFile {
                            self.file_normal_fallback = false;
                        }
                    }
                }
            }
        }
    }

    fn has_explicit_style_for(&self, indicator: Indicator) -> bool {
        self.indicator_mapping.contains_key(&indicator)
    }

    /// Whether styling a file needs its permission bits.
    pub fn needs_mode(&self) -> bool {
        [
            Indicator::Setuid,
            Indicator::Setgid,
            Indicator::ExecutableFile,
            Indicator::StickyAndOtherWritable,
            Indicator::OtherWritable,
            Indicator::Sticky,
        ]
        .iter()
        .any(|&i| self.has_explicit_style_for(i))
    }

    /// Whether styling a symbolic link needs to know if its target exists.
    pub fn needs_target(&self) -> bool {
        self.has_explicit_style_for(Indicator::OrphanedSymbolicLink)
    }

    fn indicator_for(&self, f: &Facts<'_>) -> Indicator {
        let Some(kind) = f.kind else {
            // Default to a regular file, so we still try the suffix map
            return Indicator::RegularFile;
        };
        let has = |i| self.has_explicit_style_for(i);
        match kind {
            FileKind::File => {
                if let Some(mode) = f.mode {
                    if has(Indicator::Setuid) && mode & 0o4000 != 0 {
                        return Indicator::Setuid;
                    } else if has(Indicator::Setgid) && mode & 0o2000 != 0 {
                        return Indicator::Setgid;
                    } else if has(Indicator::ExecutableFile) && mode & 0o0111 != 0 {
                        return Indicator::ExecutableFile;
                    }
                    // psh: no link counts, so no `mh`.
                }
                Indicator::RegularFile
            }
            FileKind::Dir => {
                if let Some(mode) = f.mode {
                    if has(Indicator::StickyAndOtherWritable) && mode & 0o1002 == 0o1002 {
                        return Indicator::StickyAndOtherWritable;
                    } else if has(Indicator::OtherWritable) && mode & 0o0002 != 0 {
                        return Indicator::OtherWritable;
                    } else if has(Indicator::Sticky) && mode & 0o1000 != 0 {
                        return Indicator::Sticky;
                    }
                }
                Indicator::Directory
            }
            FileKind::Symlink => {
                if has(Indicator::OrphanedSymbolicLink) && !f.target_exists {
                    return Indicator::OrphanedSymbolicLink;
                }
                Indicator::SymbolicLink
            }
            FileKind::Fifo => Indicator::Fifo,
            FileKind::Socket => Indicator::Socket,
            FileKind::BlockDevice => Indicator::BlockDevice,
            FileKind::CharDevice => Indicator::CharacterDevice,
            // Treat files of unknown type as errors
            FileKind::Other => Indicator::MissingFile,
        }
    }

    /// The style of an entry.
    pub fn style_for(&self, f: &Facts<'_>) -> Option<&Style> {
        let indicator = self.indicator_for(f);
        if indicator == Indicator::RegularFile {
            // Note: using '.to_str()' here means that filename
            // matching will not work with invalid-UTF-8 paths.
            std::str::from_utf8(f.name).ok()?;
            if let Some(style) = self.style_for_suffix(f.name) {
                return Some(style);
            }
        }
        self.style_for_indicator(indicator)
    }

    /// The latest suffix definition matching `name`; `None` inside when it has no style.
    fn style_for_suffix(&self, name: &[u8]) -> Option<&Style> {
        let lower = name.to_ascii_lowercase();
        self.suffixes
            .iter()
            .rev()
            .find(|(s, _)| {
                name.ends_with(s) || {
                    let folded = s.to_ascii_lowercase();
                    !self.case_sensitive.contains(&folded) && lower.ends_with(&folded)
                }
            })
            .and_then(|(_, style)| style.as_ref())
    }

    /// Get the ANSI style for a certain `Indicator` (regular file, directory, symlink, ...). Note
    /// that this function implements a fallback logic for some of the indicators (just like `ls`).
    pub fn style_for_indicator(&self, indicator: Indicator) -> Option<&Style> {
        self.indicator_mapping
            .get(&indicator)
            .or_else(|| {
                self.indicator_mapping.get(&match indicator {
                    Indicator::Setuid
                    | Indicator::Setgid
                    | Indicator::ExecutableFile
                    | Indicator::MultipleHardLinks => Indicator::RegularFile,

                    Indicator::StickyAndOtherWritable
                    | Indicator::OtherWritable
                    | Indicator::Sticky => Indicator::Directory,

                    Indicator::OrphanedSymbolicLink => Indicator::SymbolicLink,

                    Indicator::MissingFile => Indicator::OrphanedSymbolicLink,

                    _ => indicator,
                })
            })
            .or_else(|| {
                if indicator == Indicator::RegularFile && !self.file_normal_fallback {
                    None
                } else {
                    self.indicator_mapping.get(&Indicator::Normal)
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file<'a>(name: &'a str, kind: FileKind, mode: u32) -> Facts<'a> {
        Facts {
            name: name.as_bytes(),
            kind: Some(kind),
            mode: Some(mode),
            target_exists: false,
        }
    }

    fn prefix(c: &LsColors, f: &Facts<'_>) -> String {
        c.style_for(f).map(|s| s.prefix()).unwrap_or_default()
    }

    #[test]
    fn rendering_as_nu_ansi_term() {
        let s = Style::from_ansi_sequence("0;38;5;16;48;5;203").unwrap();
        assert_eq!(s.prefix(), "\x1b[48;5;203;38;5;16m");
        assert_eq!(
            Style::from_ansi_sequence("01;34").unwrap().prefix(),
            "\x1b[1;34m"
        );
        assert_eq!(
            Style::from_ansi_sequence("91").unwrap().prefix(),
            "\x1b[38;5;9m"
        );
        assert_eq!(
            Style::from_ansi_sequence("4").unwrap().paint("/"),
            "\x1b[4m/\x1b[0m"
        );
        assert_eq!(Style::from_ansi_sequence("0"), None);
    }

    #[test]
    fn fd_defaults() {
        let c = LsColors::for_fd(None);
        assert_eq!(
            prefix(&c, &file("a.rs", FileKind::File, 0o644)),
            "\x1b[38;5;48m"
        );
        // The type comes before the suffix.
        assert_eq!(
            prefix(&c, &file("run.sh", FileKind::File, 0o755)),
            "\x1b[1;38;5;203m"
        );
        assert_eq!(
            prefix(&c, &file("run", FileKind::File, 0o755)),
            "\x1b[1;38;5;203m"
        );
        assert_eq!(
            prefix(&c, &file("x.TXT", FileKind::File, 0o644)),
            "\x1b[38;5;185m"
        );
        assert_eq!(prefix(&c, &file("plain", FileKind::File, 0o644)), "");
        assert_eq!(
            prefix(&c, &file("d", FileKind::Dir, 0o755)),
            "\x1b[38;5;81m"
        );
        // The newline inside fd's string makes `*.xml` unstyled.
        assert_eq!(prefix(&c, &file("a.xml", FileKind::File, 0o644)), "");
        let dangling = file("l", FileKind::Symlink, 0o777);
        assert_eq!(prefix(&c, &dangling), "\x1b[48;5;203;38;5;16m");
    }

    #[test]
    fn suffix_precedence() {
        let c = LsColors::from_string("*.gz=01;31:*.tar.gz=01;33:");
        assert_eq!(
            prefix(&c, &file("a.tar.gz", FileKind::File, 0)),
            "\x1b[1;33m"
        );
        assert_eq!(prefix(&c, &file("a.GZ", FileKind::File, 0)), "\x1b[1;31m");
        let c = LsColors::from_string("*.tar.gz=01;33:*.gz=01;31:");
        assert_eq!(
            prefix(&c, &file("a.tar.gz", FileKind::File, 0)),
            "\x1b[1;31m"
        );
        let c = LsColors::from_string("*README=01:*readme=00:");
        assert_eq!(prefix(&c, &file("README", FileKind::File, 0)), "\x1b[1m");
        assert_eq!(prefix(&c, &file("readme", FileKind::File, 0)), "");
    }
}
