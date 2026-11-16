//! Syntax colouring: a deliberately small hand-rolled lexer rather than tree-sitter. It needs no grammars on disk,
//! tokenizes about a million lines a second, and a diff viewer only needs the broad strokes.
//!
//! Spans are in UTF-8 byte offsets of the line (the Mac version converts to UTF-16 for CoreText; here the
//! drawing backend converts when it needs to).

use std::collections::HashSet;
use std::sync::LazyLock;

/// Semantic class of a highlighted run. The value indexes the theme's colour table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TokenKind {
    Plain = 0,
    Keyword,
    String,
    Comment,
    Number,
    Annotation,
    Function,
    Constant,
    Tag,
    Attribute,
}

impl TokenKind {
    pub const COUNT: usize = 10;
}

/// A coloured run inside one line, in UTF-8 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyntaxSpan {
    pub start: u32,
    pub length: u32,
    pub kind: TokenKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    CLike,
    Markup,
    Yaml,
    Markdown,
    KeyValue,
    Plain,
}

/// Describes just enough of a language's lexical structure to colour it.
#[derive(Debug)]
pub struct LanguageSpec {
    pub name: &'static str,
    pub family: Family,
    pub keywords: HashSet<String>,
    pub type_keywords: HashSet<String>,
    pub def_keywords: HashSet<String>,
    pub line_comments: Vec<&'static [u8]>,
    pub block_open: &'static [u8],
    pub block_close: &'static [u8],
    pub quotes: Vec<u8>,
    pub multiline_quotes: Vec<u8>,
    pub triple_quotes: bool,
    pub annotations: bool,
    pub preprocessor: bool,
    pub dollar_vars: bool,
    pub property_keys: bool,
    pub case_insensitive: bool,
    pub decl_by_shape: bool,
    pub lifetimes: bool,
    pub go_receivers: bool,
    max_keyword_length: usize,
}

impl PartialEq for LanguageSpec {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl LanguageSpec {
    fn new(name: &'static str, family: Family) -> Self {
        LanguageSpec {
            name,
            family,
            keywords: HashSet::new(),
            type_keywords: HashSet::new(),
            def_keywords: HashSet::new(),
            line_comments: Vec::new(),
            block_open: b"",
            block_close: b"",
            quotes: vec![b'"', b'\''],
            multiline_quotes: Vec::new(),
            triple_quotes: false,
            annotations: false,
            preprocessor: false,
            dollar_vars: false,
            property_keys: false,
            case_insensitive: false,
            decl_by_shape: false,
            lifetimes: false,
            go_receivers: false,
            max_keyword_length: 0,
        }
    }

    fn keywords(mut self, list: &str) -> Self {
        self.keywords.extend(list.split_whitespace().map(str::to_string));
        self
    }

    fn types(mut self, list: &str) -> Self {
        self.type_keywords.extend(list.split_whitespace().map(str::to_string));
        self
    }

    fn defs(mut self, list: &str) -> Self {
        self.def_keywords.extend(list.split_whitespace().map(str::to_string));
        self
    }

    fn slash_comments(mut self) -> Self {
        self.line_comments = vec![b"//"];
        self.block_open = b"/*";
        self.block_close = b"*/";
        self
    }

    fn hash_comments(mut self) -> Self {
        self.line_comments = vec![b"#"];
        self
    }

    fn with(mut self, f: impl FnOnce(&mut Self)) -> Self {
        f(&mut self);
        self
    }

    fn done(mut self) -> Self {
        self.max_keyword_length =
            self.keywords.iter().chain(self.type_keywords.iter()).map(|k| k.len()).max().unwrap_or(0);
        self
    }
}

macro_rules! language {
    ($name:ident, $build:expr) => {
        pub static $name: LazyLock<LanguageSpec> = LazyLock::new(|| $build.done());
    };
}

language!(PLAIN, LanguageSpec::new("Text", Family::Plain));

language!(
    KOTLIN,
    LanguageSpec::new("Kotlin", Family::CLike)
        .keywords(
            "abstract actual annotation as break by catch class companion const constructor continue crossinline data \
             delegate do dynamic else enum expect external false final finally for fun get if import in infix init inline \
             inner interface internal is lateinit noinline null object open operator out override package private protected \
             public reified return sealed set super suspend tailrec this throw true try typealias typeof val value var \
             vararg when where while field it"
        )
        .defs("fun")
        .slash_comments()
        .with(|s| {
            s.triple_quotes = true;
            s.annotations = true;
        })
);

language!(
    JAVA,
    LanguageSpec::new("Java", Family::CLike)
        .keywords(
            "abstract assert break case catch class const continue default do else enum extends final finally for goto if \
             implements import instanceof interface native new package private protected public return static strictfp \
             super switch synchronized this throw throws transient try volatile while true false null record sealed \
             permits yield non-sealed"
        )
        .types("boolean byte char double float int long short void var")
        .slash_comments()
        .with(|s| {
            s.triple_quotes = true;
            s.annotations = true;
            s.decl_by_shape = true;
        })
);

language!(
    SWIFT,
    LanguageSpec::new("Swift", Family::CLike)
        .keywords(
            "actor any as associatedtype async await break case catch class continue convenience default defer deinit \
             didSet do dynamic else enum extension fallthrough false fileprivate final for func get guard if import in \
             indirect infix init inout internal is isolated lazy let mutating nil nonisolated nonmutating open operator \
             optional override postfix precedencegroup prefix private protocol public repeat required rethrows return \
             self Self set some static struct subscript super switch throw throws true try typealias unowned var weak \
             where while willSet consuming borrowing macro"
        )
        .defs("func")
        .slash_comments()
        .with(|s| {
            s.triple_quotes = true;
            s.annotations = true;
            s.preprocessor = true;
            s.quotes = vec![b'"'];
        })
);

language!(
    JAVASCRIPT,
    LanguageSpec::new("JavaScript", Family::CLike)
        .keywords(
            "abstract any as async await boolean break case catch class const constructor continue debugger declare \
             default delete do else enum export extends false finally for from function get if implements import in \
             infer instanceof interface is keyof let module namespace never new null number object of package private \
             protected public readonly require return satisfies set static string super switch symbol this throw true \
             try type typeof undefined unique unknown var void while with yield"
        )
        .defs("function")
        .slash_comments()
        .with(|s| {
            s.quotes = vec![b'"', b'\'', b'`'];
            s.multiline_quotes = vec![b'`'];
            s.annotations = true;
        })
);

language!(
    PYTHON,
    LanguageSpec::new("Python", Family::CLike)
        .keywords(
            "False None True and as assert async await break case class continue def del elif else except finally for \
             from global if import in is lambda match nonlocal not or pass raise return self try while with yield"
        )
        .defs("def class")
        .hash_comments()
        .with(|s| {
            s.triple_quotes = true;
            s.annotations = true;
        })
);

language!(
    GO,
    LanguageSpec::new("Go", Family::CLike)
        .keywords(
            "break case chan const continue default defer else fallthrough for func go goto if import interface map \
             package range return select struct switch type var true false nil iota any"
        )
        .types(
            "bool byte complex64 complex128 error float32 float64 int int8 int16 int32 int64 rune string uint uint8 uint16 \
             uint32 uint64 uintptr"
        )
        .defs("func")
        .slash_comments()
        .with(|s| {
            s.quotes = vec![b'"', b'\'', b'`'];
            s.multiline_quotes = vec![b'`'];
            s.go_receivers = true;
        })
);

language!(
    RUST,
    LanguageSpec::new("Rust", Family::CLike)
        .keywords(
            "as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod \
             move mut pub ref return self Self static struct super trait true type unsafe use where while union macro_rules"
        )
        .types("bool char f32 f64 i8 i16 i32 i64 i128 isize str u8 u16 u32 u64 u128 usize")
        .defs("fn")
        .slash_comments()
        .with(|s| {
            s.lifetimes = true;
        })
);

language!(
    C_FAMILY,
    LanguageSpec::new("C/C++", Family::CLike)
        .keywords(
            "alignas alignof and asm break case catch class co_await co_return co_yield concept const const_cast \
             consteval constexpr constinit continue decltype default delete do dynamic_cast else enum explicit export \
             extern false final for friend goto if inline mutable namespace new noexcept not nullptr operator or \
             override private protected public register reinterpret_cast requires return sizeof static static_assert \
             static_cast struct switch template this thread_local throw true try typedef typeid typename union using \
             virtual volatile while NULL nil self super id instancetype YES NO"
        )
        .types("auto bool char char8_t char16_t char32_t double float int long short signed unsigned void wchar_t size_t BOOL")
        .slash_comments()
        .with(|s| {
            s.preprocessor = true;
            s.annotations = true; // Objective-C @interface, @property …
            s.decl_by_shape = true;
        })
);

language!(
    CSHARP,
    LanguageSpec::new("C#", Family::CLike)
        .keywords(
            "abstract as async await base break case catch checked class const continue default delegate do else enum \
             event explicit extern false finally fixed for foreach get goto if implicit in interface internal is lock \
             namespace new null operator out override params partial private protected public readonly record ref \
             return sealed set sizeof stackalloc static struct switch this throw true try typeof unchecked unsafe using \
             virtual volatile when where while yield"
        )
        .types("bool byte char decimal double dynamic float int long object sbyte short string uint ulong ushort var void")
        .slash_comments()
        .with(|s| {
            s.preprocessor = true;
            s.decl_by_shape = true;
        })
);

language!(
    DART,
    LanguageSpec::new("Dart", Family::CLike)
        .keywords(
            "abstract as assert async await base break case catch class const continue covariant default deferred do \
             else enum export extends extension external factory false final finally for get hide if implements import \
             in interface is late library mixin new null on operator part required rethrow return sealed set show \
             static super switch sync this throw true try typedef when while with yield"
        )
        .types("bool double dynamic int num var void")
        .slash_comments()
        .with(|s| {
            s.triple_quotes = true;
            s.annotations = true;
            s.decl_by_shape = true;
        })
);

language!(
    GROOVY,
    LanguageSpec::new("Groovy", Family::CLike)
        .keywords(
            "abstract as assert break case catch class const continue def default do else enum extends false final \
             finally for goto if implements import in instanceof interface native new null package private protected \
             public return static super switch synchronized this throw throws trait transient true try volatile while"
        )
        .types("boolean byte char double float int long short void var")
        .defs("def")
        .slash_comments()
        .with(|s| {
            s.triple_quotes = true;
            s.annotations = true;
        })
);

language!(
    RUBY,
    LanguageSpec::new("Ruby", Family::CLike)
        .keywords(
            "alias and begin break case class def defined? do else elsif end ensure false for if in module next nil not \
             or redo rescue retry return self super then true undef unless until when while yield require include \
             attr_accessor"
        )
        .defs("def class module")
        .hash_comments()
);

language!(
    PHP,
    LanguageSpec::new("PHP", Family::CLike)
        .keywords(
            "abstract and array as break callable case catch class clone const continue declare default do echo else \
             elseif empty enddeclare endfor endforeach endif endswitch endwhile enum extends false final finally fn for \
             foreach function global goto if implements include include_once instanceof insteadof interface isset list \
             match namespace new null or print private protected public readonly require require_once return static \
             switch throw trait true try unset use var while xor yield"
        )
        .defs("function")
        .with(|s| {
            s.line_comments = vec![b"//", b"#"];
            s.block_open = b"/*";
            s.block_close = b"*/";
            s.dollar_vars = true;
        })
);

language!(
    SHELL,
    LanguageSpec::new("Shell", Family::CLike)
        .keywords(
            "if then else elif fi for while until do done case esac function in select time return exit export local \
             readonly declare unset shift source alias echo printf read set cd eval exec trap true false"
        )
        .hash_comments()
        .with(|s| {
            s.dollar_vars = true;
            s.quotes = vec![b'"', b'\'', b'`'];
            s.multiline_quotes = vec![b'"', b'\''];
        })
);

language!(
    POWERSHELL,
    LanguageSpec::new("PowerShell", Family::CLike)
        .keywords(
            "begin break catch class continue data define do dynamicparam else elseif end enum exit filter finally for \
             foreach from function hidden if in param process return static switch throw trap try until using var \
             while true false null"
        )
        .defs("function")
        .hash_comments()
        .with(|s| {
            s.dollar_vars = true;
            s.case_insensitive = true;
            s.block_open = b"<#";
            s.block_close = b"#>";
        })
);

language!(
    SQL,
    LanguageSpec::new("SQL", Family::CLike)
        .keywords(
            "add all alter and as asc begin between by case check column commit constraint create cross database default \
             delete desc distinct drop else end exists foreign from full group having if in index inner insert into is \
             join key left like limit not null offset on or order outer primary references right rollback select set \
             table then transaction union unique update values view when where with"
        )
        .types("bigint blob boolean char date datetime decimal double float int integer real text time timestamp varchar")
        .with(|s| {
            s.line_comments = vec![b"--"];
            s.block_open = b"/*";
            s.block_close = b"*/";
            s.case_insensitive = true;
        })
);

language!(
    CSS,
    LanguageSpec::new("CSS", Family::CLike).keywords("important inherit initial unset none auto").slash_comments().with(|s| {
        s.annotations = true;
    })
);

language!(
    LUA,
    LanguageSpec::new("Lua", Family::CLike)
        .keywords(
            "and break do else elseif end false for function goto if in local nil not or repeat return then true until while"
        )
        .defs("function")
        .with(|s| {
            s.line_comments = vec![b"--"];
        })
);

language!(
    PROTO,
    LanguageSpec::new("Protobuf", Family::CLike)
        .keywords(
            "syntax package import option message enum service rpc returns repeated optional required oneof map reserved \
             extend extensions stream true false public weak"
        )
        .types(
            "bool bytes double fixed32 fixed64 float int32 int64 sfixed32 sfixed64 sint32 sint64 string uint32 uint64"
        )
        .slash_comments()
);

language!(
    JSON,
    LanguageSpec::new("JSON", Family::CLike).keywords("true false null").slash_comments().with(|s| {
        s.quotes = vec![b'"'];
        s.property_keys = true;
    })
);

language!(
    CMAKE,
    LanguageSpec::new("CMake", Family::CLike)
        .keywords(
            "if else elseif endif foreach endforeach while endwhile function endfunction macro endmacro set return include \
             ON OFF TRUE FALSE AND OR NOT"
        )
        .hash_comments()
        .with(|s| {
            s.dollar_vars = true;
            s.quotes = vec![b'"'];
        })
);

language!(
    BATCH,
    LanguageSpec::new("Batch", Family::CLike)
        .keywords(
            "if else for in do goto call exit set setlocal endlocal echo off on not exist defined errorlevel shift pause \
             rem start cd pushd popd"
        )
        .with(|s| {
            s.line_comments = vec![b"::", b"rem ", b"REM "];
            s.case_insensitive = true;
            s.quotes = vec![b'"'];
        })
);

language!(XML, LanguageSpec::new("XML", Family::Markup));
language!(YAML, LanguageSpec::new("YAML", Family::Yaml));
language!(MARKDOWN, LanguageSpec::new("Markdown", Family::Markdown));
language!(KEY_VALUE, LanguageSpec::new("Config", Family::KeyValue));

fn by_extension(ext: &str) -> Option<&'static LanguageSpec> {
    let spec: &'static LazyLock<LanguageSpec> = match ext {
        "kt" | "kts" => &KOTLIN,
        "java" | "aidl" => &JAVA,
        "swift" => &SWIFT,
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "mts" | "cts" | "vue" | "svelte" => &JAVASCRIPT,
        "py" | "pyi" | "pyw" | "bzl" | "bazel" => &PYTHON,
        "go" => &GO,
        "rs" => &RUST,
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "m" | "mm" | "inl" | "metal" | "glsl" | "vert"
        | "frag" | "hlsl" => &C_FAMILY,
        "cs" => &CSHARP,
        "dart" => &DART,
        "gradle" | "groovy" => &GROOVY,
        "rb" | "podspec" | "gemspec" | "rake" => &RUBY,
        "php" => &PHP,
        "sh" | "bash" | "zsh" | "fish" | "command" => &SHELL,
        "ps1" | "psm1" | "psd1" => &POWERSHELL,
        "bat" | "cmd" => &BATCH,
        "sql" => &SQL,
        "css" | "scss" | "less" | "sass" => &CSS,
        "lua" => &LUA,
        "proto" => &PROTO,
        "json" | "json5" | "jsonc" | "webmanifest" | "map" | "arb" => &JSON,
        "cmake" => &CMAKE,
        "xml" | "html" | "htm" | "xhtml" | "svg" | "plist" | "storyboard" | "xib" | "iml" | "xsd" | "xsl" | "xslt"
        | "pom" | "csproj" | "jsp" | "vsdx" | "xaml" | "vcxproj" | "props" | "targets" | "resx" | "manifest" => &XML,
        "yaml" | "yml" => &YAML,
        "md" | "markdown" | "mdx" => &MARKDOWN,
        "properties" | "ini" | "toml" | "cfg" | "conf" | "env" | "pro" | "gitconfig" | "editorconfig" | "lock" => {
            &KEY_VALUE
        }
        _ => return None,
    };
    Some(spec)
}

fn by_file_name(name: &str) -> Option<&'static LanguageSpec> {
    let spec: &'static LazyLock<LanguageSpec> = match name {
        "Podfile" | "Gemfile" | "Fastfile" | "Rakefile" | "Appfile" => &RUBY,
        "Dockerfile" | "Makefile" | "makefile" | "gradlew" => &SHELL,
        "Jenkinsfile" => &GROOVY,
        "CMakeLists.txt" => &CMAKE,
        ".gitignore" | ".gitattributes" | ".gitmodules" => &KEY_VALUE,
        ".zshrc" | ".bashrc" | ".bash_profile" => &SHELL,
        "BUILD" | "WORKSPACE" => &PYTHON,
        _ => return None,
    };
    Some(spec)
}

/// The language a path's name or extension says it is.
pub fn detect(path: &str) -> &'static LanguageSpec {
    let name = crate::util::file_name(path);
    if let Some(spec) = by_file_name(name) {
        return spec;
    }
    let ext = crate::util::extension(name);
    if let Some(spec) = by_extension(&ext) {
        return spec;
    }
    if name.starts_with(".env") {
        return &KEY_VALUE;
    }
    if name.starts_with("Dockerfile") {
        return &SHELL;
    }
    &PLAIN
}

// MARK: - Lexer

/// Lines longer than this are left uncoloured; they are almost always minified or generated.
pub const MAX_LINE_BYTES: usize = 4000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Normal,
    BlockComment,
    String,
    MarkupComment,
    MarkupTag,
    MarkupString,
    Cdata,
    Fence,
}

/// What the lexer carries from one line into the next: an open comment, string or fence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct State {
    pub mode: Mode,
    pub delimiter: u8,
    pub triple: bool,
}

pub fn highlight<S: AsRef<str>>(lines: &[S], language: &LanguageSpec) -> Vec<Vec<SyntaxSpan>> {
    if language.family == Family::Plain {
        return Vec::new();
    }
    let mut result = vec![Vec::new(); lines.len()];
    let mut state = State::default();
    let mut spans = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        scan(line.as_ref(), language, &mut state, &mut spans);
        if !spans.is_empty() {
            result[index] = spans.clone();
        }
    }
    result
}

/// As `highlight`, also giving the lexer state at the start of every line and after the last one,
/// which is what lets an edit re-colour only the lines it affects.
pub fn highlight_tracking<S: AsRef<str>>(lines: &[S], language: &LanguageSpec) -> (Vec<Vec<SyntaxSpan>>, Vec<State>) {
    let mut result = vec![Vec::new(); lines.len()];
    let mut states = Vec::with_capacity(lines.len() + 1);
    let mut state = State::default();
    let mut spans = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        states.push(state);
        if language.family == Family::Plain {
            continue;
        }
        scan(line.as_ref(), language, &mut state, &mut spans);
        if !spans.is_empty() {
            result[index] = spans.clone();
        }
    }
    states.push(state);
    (result, states)
}

/// Re-colours lines `first..first + count`, which changed, and then the lines after them for as long as
/// the lexer state coming into a line differs from what it was before (a comment opened or closed).
///
/// `spans` and `states` must already have the new number of lines (`states` one more); the entries for
/// the changed lines can be anything. Returns the lines whose colours were recomputed.
pub fn rehighlight<S: AsRef<str>>(
    lines: &[S],
    language: &LanguageSpec,
    first: usize,
    count: usize,
    spans: &mut [Vec<SyntaxSpan>],
    states: &mut [State],
) -> std::ops::Range<usize> {
    if language.family == Family::Plain
        || first >= lines.len()
        || spans.len() != lines.len()
        || states.len() != lines.len() + 1
    {
        return first..first;
    }
    let mut state = states[first];
    let mut line = first;
    let mut scratch = Vec::new();
    while line < lines.len() {
        if line >= first + count && states[line] == state {
            break;
        }
        states[line] = state;
        scan(lines[line].as_ref(), language, &mut state, &mut scratch);
        spans[line] = scratch.clone();
        line += 1;
    }
    if line == lines.len() {
        states[lines.len()] = state;
    }
    first..line
}

/// Colours one line, carrying the lexer state over from the line before. Leaves the spans in `spans`.
pub fn scan(line: &str, language: &LanguageSpec, state: &mut State, spans: &mut Vec<SyntaxSpan>) {
    spans.clear();
    if line.is_empty() {
        return;
    }
    let buf = line.as_bytes();
    if buf.len() > MAX_LINE_BYTES {
        return;
    }
    match language.family {
        Family::CLike => scan_c_like(buf, language, state, spans),
        Family::Markup => scan_markup(buf, state, spans),
        Family::Yaml => scan_yaml(buf, spans),
        Family::Markdown => scan_markdown(buf, state, spans),
        Family::KeyValue => scan_key_value(buf, spans),
        Family::Plain => {}
    }
}

#[inline(always)]
fn is_digit(c: u8) -> bool {
    c.is_ascii_digit()
}

#[inline(always)]
fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c >= 0x80
}

#[inline(always)]
fn is_ident_part(c: u8) -> bool {
    is_ident_start(c) || is_digit(c)
}

#[inline(always)]
fn is_space(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

#[inline(always)]
fn emit(spans: &mut Vec<SyntaxSpan>, start: usize, end: usize, kind: TokenKind) {
    if end > start {
        spans.push(SyntaxSpan { start: start as u32, length: (end - start) as u32, kind });
    }
}

fn matches(buf: &[u8], at: usize, pattern: &[u8]) -> bool {
    !pattern.is_empty() && at + pattern.len() <= buf.len() && &buf[at..at + pattern.len()] == pattern
}

fn find(buf: &[u8], from: usize, pattern: &[u8]) -> Option<usize> {
    if pattern.is_empty() || buf.len() < pattern.len() {
        return None;
    }
    let last = buf.len() - pattern.len();
    let mut i = from;
    while i <= last {
        if buf[i] == pattern[0] && matches(buf, i, pattern) {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Index just past the closing delimiter of a string starting after `from`, or None if it runs off the line.
fn string_end(buf: &[u8], from: usize, delimiter: u8, triple: bool) -> Option<usize> {
    let n = buf.len();
    let mut j = from;
    while j < n {
        let c = buf[j];
        if c == b'\\' {
            j += 2;
            continue;
        }
        if c == delimiter {
            if !triple {
                return Some(j + 1);
            }
            if j + 2 < n && buf[j + 1] == delimiter && buf[j + 2] == delimiter {
                return Some(j + 3);
            }
        }
        j += 1;
    }
    None
}

// MARK: C-like

fn scan_c_like(buf: &[u8], spec: &LanguageSpec, state: &mut State, spans: &mut Vec<SyntaxSpan>) {
    let n = buf.len();
    let mut i = 0;

    match state.mode {
        Mode::BlockComment => {
            if let Some(close) = find(buf, 0, spec.block_close) {
                i = close + spec.block_close.len();
                emit(spans, 0, i, TokenKind::Comment);
                state.mode = Mode::Normal;
            } else {
                emit(spans, 0, n, TokenKind::Comment);
                return;
            }
        }
        Mode::String => {
            if let Some(end) = string_end(buf, 0, state.delimiter, state.triple) {
                emit(spans, 0, end, TokenKind::String);
                i = end;
                state.mode = Mode::Normal;
            } else {
                emit(spans, 0, n, TokenKind::String);
                return;
            }
        }
        _ => state.mode = Mode::Normal,
    }

    let mut expect_name = false; // the next identifier names a declaration (`fun foo`)
    let mut previous_typeish = false; // previous token could be a return type (`String foo(`)
    let mut saw_content = false;

    while i < n {
        let c = buf[i];
        if is_space(c) {
            i += 1;
            continue;
        }

        if spec.line_comments.iter().any(|p| matches(buf, i, p)) {
            emit(spans, i, n, TokenKind::Comment);
            return;
        }
        if matches(buf, i, spec.block_open) {
            if let Some(close) = find(buf, i + spec.block_open.len(), spec.block_close) {
                let end = close + spec.block_close.len();
                emit(spans, i, end, TokenKind::Comment);
                i = end;
                continue;
            }
            emit(spans, i, n, TokenKind::Comment);
            state.mode = Mode::BlockComment;
            return;
        }

        if spec.quotes.contains(&c) {
            // Rust lifetimes (`'a`) look like an unterminated char literal.
            if spec.lifetimes && c == b'\'' {
                let is_char = (i + 2 < n && buf[i + 2] == b'\'') || (i + 1 < n && buf[i + 1] == b'\\');
                if !is_char {
                    i += 1;
                    saw_content = true;
                    continue;
                }
            }
            let triple = spec.triple_quotes && i + 2 < n && buf[i + 1] == c && buf[i + 2] == c;
            let body_start = if triple { i + 3 } else { i + 1 };
            if let Some(end) = string_end(buf, body_start, c, triple) {
                let mut kind = TokenKind::String;
                if spec.property_keys {
                    let mut k = end;
                    while k < n && is_space(buf[k]) {
                        k += 1;
                    }
                    if k < n && buf[k] == b':' {
                        kind = TokenKind::Attribute;
                    }
                }
                emit(spans, i, end, kind);
                i = end;
            } else {
                emit(spans, i, n, TokenKind::String);
                if triple || spec.multiline_quotes.contains(&c) {
                    state.mode = Mode::String;
                    state.delimiter = c;
                    state.triple = triple;
                }
                return;
            }
            previous_typeish = false;
            saw_content = true;
            continue;
        }

        if is_digit(c) || (c == b'.' && i + 1 < n && is_digit(buf[i + 1])) {
            let mut j = i + 1;
            while j < n {
                let d = buf[j];
                if is_ident_part(d) && d < 0x80 {
                    j += 1;
                    continue;
                }
                if d == b'.' && j + 1 < n && is_digit(buf[j + 1]) {
                    j += 1;
                    continue;
                }
                break;
            }
            emit(spans, i, j, TokenKind::Number);
            i = j;
            previous_typeish = false;
            saw_content = true;
            continue;
        }

        if is_ident_start(c) {
            let mut j = i + 1;
            while j < n && is_ident_part(buf[j]) {
                j += 1;
            }
            let length = j - i;
            let mut kind = TokenKind::Plain;
            let mut typeish = true;
            if length <= spec.max_keyword_length {
                let raw = std::str::from_utf8(&buf[i..j]).unwrap_or("");
                let lowered;
                let word = if spec.case_insensitive {
                    lowered = raw.to_lowercase();
                    lowered.as_str()
                } else {
                    raw
                };
                if spec.keywords.contains(word) {
                    kind = TokenKind::Keyword;
                    typeish = false;
                    if spec.def_keywords.contains(word) {
                        expect_name = true;
                        emit(spans, i, j, kind);
                        i = j;
                        previous_typeish = false;
                        saw_content = true;
                        continue;
                    }
                } else if spec.type_keywords.contains(word) {
                    kind = TokenKind::Keyword;
                }
            }
            if kind == TokenKind::Plain {
                let mut k = j;
                while k < n && is_space(buf[k]) {
                    k += 1;
                }
                let next = if k < n { buf[k] } else { 0 };
                if expect_name {
                    // `fun Receiver.name()` — keep waiting for the real name after the dot.
                    if next == b'.' || next == b'<' {
                        typeish = false;
                    } else {
                        kind = TokenKind::Function;
                        expect_name = false;
                    }
                } else if spec.decl_by_shape && previous_typeish && next == b'(' {
                    kind = TokenKind::Function;
                } else if length >= 2 && is_constant_name(buf, i, j) {
                    kind = TokenKind::Constant;
                }
            }
            if kind != TokenKind::Plain {
                emit(spans, i, j, kind);
            }
            previous_typeish = typeish;
            i = j;
            saw_content = true;
            continue;
        }

        if c == b'@' && spec.annotations && i + 1 < n && is_ident_start(buf[i + 1]) {
            let mut j = i + 1;
            while j < n && (is_ident_part(buf[j]) || buf[j] == b'.' || buf[j] == b':') {
                j += 1;
            }
            emit(spans, i, j, TokenKind::Annotation);
            i = j;
            saw_content = true;
            continue;
        }

        if c == b'#' && spec.preprocessor && !saw_content {
            let mut j = i + 1;
            while j < n && is_space(buf[j]) {
                j += 1;
            }
            while j < n && is_ident_part(buf[j]) {
                j += 1;
            }
            emit(spans, i, j, TokenKind::Annotation);
            i = j;
            saw_content = true;
            continue;
        }

        if c == b'$' && spec.dollar_vars && i + 1 < n {
            let mut j = i + 1;
            if buf[j] == b'{' {
                while j < n && buf[j] != b'}' {
                    j += 1;
                }
                if j < n {
                    j += 1;
                }
            } else {
                while j < n && is_ident_part(buf[j]) {
                    j += 1;
                }
            }
            if j > i + 1 {
                emit(spans, i, j, TokenKind::Constant);
            }
            i = j.max(i + 1);
            saw_content = true;
            continue;
        }

        // Punctuation.
        if expect_name {
            if c == b'<' {
                // skip generic parameters: `fun <T> name()`
                let mut j = i + 1;
                while j < n && buf[j] != b'>' {
                    j += 1;
                }
                i = n.min(j + 1);
                continue;
            }
            if c == b'(' {
                if spec.go_receivers {
                    // func (r *Recv) Name()
                    let mut j = i + 1;
                    while j < n && buf[j] != b')' {
                        j += 1;
                    }
                    i = n.min(j + 1);
                    continue;
                }
                expect_name = false;
            } else if c != b'.' && c != b'*' && c != b'&' {
                expect_name = false;
            }
        }
        // `>` and `]` close generic and array types, `*`/`&` decorate pointer types.
        previous_typeish = ((c == b'>' || c == b']' || c == b'*' || c == b'&') && previous_typeish) || (c == b'>' || c == b']');
        saw_content = true;
        i += 1;
    }
}

/// SCREAMING_SNAKE identifiers read as constants.
fn is_constant_name(buf: &[u8], from: usize, to: usize) -> bool {
    let mut letters = 0;
    for &c in &buf[from..to] {
        if c.is_ascii_uppercase() {
            letters += 1;
        } else if !(is_digit(c) || c == b'_') {
            return false;
        }
    }
    letters >= 2
}

// MARK: XML / HTML

const COMMENT_OPEN: &[u8] = b"<!--";
const COMMENT_CLOSE: &[u8] = b"-->";
const CDATA_OPEN: &[u8] = b"<![CDATA[";
const CDATA_CLOSE: &[u8] = b"]]>";

#[inline(always)]
fn is_name_char(c: u8) -> bool {
    is_ident_part(c) || c == b'-' || c == b'.' || c == b':'
}

fn scan_markup(buf: &[u8], state: &mut State, spans: &mut Vec<SyntaxSpan>) {
    let n = buf.len();
    let mut i = 0;
    if !matches!(state.mode, Mode::MarkupComment | Mode::MarkupTag | Mode::MarkupString | Mode::Cdata) {
        state.mode = Mode::Normal;
    }
    while i < n {
        match state.mode {
            Mode::MarkupComment => {
                if let Some(close) = find(buf, i, COMMENT_CLOSE) {
                    emit(spans, i, close + 3, TokenKind::Comment);
                    i = close + 3;
                    state.mode = Mode::Normal;
                } else {
                    emit(spans, i, n, TokenKind::Comment);
                    return;
                }
            }
            Mode::Cdata => {
                if let Some(close) = find(buf, i, CDATA_CLOSE) {
                    emit(spans, close, close + 3, TokenKind::Tag);
                    i = close + 3;
                    state.mode = Mode::Normal;
                } else {
                    return;
                }
            }
            Mode::MarkupString => {
                let mut j = i;
                while j < n && buf[j] != state.delimiter {
                    j += 1;
                }
                if j < n {
                    emit(spans, i, j + 1, TokenKind::String);
                    i = j + 1;
                    state.mode = Mode::MarkupTag;
                } else {
                    emit(spans, i, n, TokenKind::String);
                    return;
                }
            }
            Mode::MarkupTag => {
                let c = buf[i];
                if is_space(c) {
                    i += 1;
                    continue;
                }
                if c == b'>' {
                    emit(spans, i, i + 1, TokenKind::Tag);
                    i += 1;
                    state.mode = Mode::Normal;
                } else if (c == b'/' || c == b'?') && i + 1 < n && buf[i + 1] == b'>' {
                    emit(spans, i, i + 2, TokenKind::Tag);
                    i += 2;
                    state.mode = Mode::Normal;
                } else if c == b'"' || c == b'\'' {
                    state.delimiter = c;
                    let mut j = i + 1;
                    while j < n && buf[j] != c {
                        j += 1;
                    }
                    if j < n {
                        emit(spans, i, j + 1, TokenKind::String);
                        i = j + 1;
                    } else {
                        emit(spans, i, n, TokenKind::String);
                        state.mode = Mode::MarkupString;
                        return;
                    }
                } else if is_name_char(c) {
                    let mut j = i;
                    let mut colon: Option<usize> = None;
                    while j < n && is_name_char(buf[j]) {
                        if buf[j] == b':' && colon.is_none() {
                            colon = Some(j);
                        }
                        j += 1;
                    }
                    match colon {
                        Some(col) if col > i => {
                            // namespace prefix, e.g. android:
                            emit(spans, i, col + 1, TokenKind::Constant);
                            emit(spans, col + 1, j, TokenKind::Attribute);
                        }
                        _ => emit(spans, i, j, TokenKind::Attribute),
                    }
                    i = j;
                } else {
                    i += 1;
                }
            }
            _ => {
                let c = buf[i];
                if c == b'<' {
                    if matches(buf, i, COMMENT_OPEN) {
                        state.mode = Mode::MarkupComment;
                        continue;
                    }
                    if matches(buf, i, CDATA_OPEN) {
                        emit(spans, i, i + CDATA_OPEN.len(), TokenKind::Tag);
                        i += CDATA_OPEN.len();
                        state.mode = Mode::Cdata;
                        continue;
                    }
                    let mut j = i + 1;
                    if j < n && (buf[j] == b'/' || buf[j] == b'?' || buf[j] == b'!') {
                        j += 1;
                    }
                    while j < n && is_name_char(buf[j]) {
                        j += 1;
                    }
                    emit(spans, i, j, TokenKind::Tag);
                    i = j;
                    state.mode = Mode::MarkupTag;
                } else if c == b'&' {
                    let mut j = i + 1;
                    while j < n && j - i < 12 && buf[j] != b';' && !is_space(buf[j]) {
                        j += 1;
                    }
                    if j < n && buf[j] == b';' {
                        emit(spans, i, j + 1, TokenKind::Constant);
                        i = j + 1;
                    } else {
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            }
        }
    }
}

// MARK: YAML

fn scan_yaml(buf: &[u8], spans: &mut Vec<SyntaxSpan>) {
    let n = buf.len();
    let mut i = 0;
    while i < n && is_space(buf[i]) {
        i += 1;
    }
    while i + 1 < n && buf[i] == b'-' && is_space(buf[i + 1]) {
        i += 2;
        while i < n && is_space(buf[i]) {
            i += 1;
        }
    }
    if i >= n {
        return;
    }
    if buf[i] == b'#' {
        emit(spans, i, n, TokenKind::Comment);
        return;
    }
    if buf[i] != b'"' && buf[i] != b'\'' {
        let mut j = i;
        while j < n {
            if buf[j] == b':' && (j + 1 == n || is_space(buf[j + 1])) {
                break;
            }
            if buf[j] == b'#' && j > 0 && is_space(buf[j - 1]) {
                j = n;
                break;
            }
            j += 1;
        }
        if j < n {
            emit(spans, i, j, TokenKind::Keyword);
            i = j + 1;
        }
    }
    scan_scalars(buf, i, spans);
}

/// Colours strings, numbers, booleans and trailing `#` comments in a value position.
fn scan_scalars(buf: &[u8], from: usize, spans: &mut Vec<SyntaxSpan>) {
    let n = buf.len();
    let mut i = from;
    while i < n {
        let c = buf[i];
        if is_space(c) {
            i += 1;
            continue;
        }
        if c == b'#' && (i == 0 || is_space(buf[i - 1])) {
            emit(spans, i, n, TokenKind::Comment);
            return;
        }
        if c == b'"' || c == b'\'' {
            let end = string_end(buf, i + 1, c, false).unwrap_or(n).min(n);
            emit(spans, i, end, TokenKind::String);
            i = end;
            continue;
        }
        if (c == b'&' || c == b'*') && i + 1 < n && is_ident_start(buf[i + 1]) {
            let mut j = i + 1;
            while j < n && (is_ident_part(buf[j]) || buf[j] == b'-') {
                j += 1;
            }
            emit(spans, i, j, TokenKind::Annotation);
            i = j;
            continue;
        }
        let mut j = i;
        while j < n && !is_space(buf[j]) && buf[j] != b',' && buf[j] != b']' && buf[j] != b'}' {
            j += 1;
        }
        if j > i {
            if is_number_token(buf, i, j) {
                emit(spans, i, j, TokenKind::Number);
            } else if j - i <= 5 {
                let word = String::from_utf8_lossy(&buf[i..j]).to_lowercase();
                if ["true", "false", "null", "yes", "no", "on", "off", "~"].contains(&word.as_str()) {
                    emit(spans, i, j, TokenKind::Keyword);
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
}

fn is_number_token(buf: &[u8], from: usize, to: usize) -> bool {
    let mut digits = 0;
    for &c in &buf[from..to] {
        if is_digit(c) {
            digits += 1;
        } else if !matches!(c, b'.' | b'-' | b'+' | b'_' | b'e' | b'x') {
            return false;
        }
    }
    digits > 0
}

// MARK: Markdown

fn scan_markdown(buf: &[u8], state: &mut State, spans: &mut Vec<SyntaxSpan>) {
    let n = buf.len();
    let mut i = 0;
    while i < n && is_space(buf[i]) {
        i += 1;
    }
    let is_fence = i + 2 < n && (buf[i] == b'`' || buf[i] == b'~') && buf[i + 1] == buf[i] && buf[i + 2] == buf[i];
    if state.mode == Mode::Fence {
        emit(spans, 0, n, TokenKind::String);
        if is_fence {
            state.mode = Mode::Normal;
        }
        return;
    }
    state.mode = Mode::Normal;
    if is_fence {
        emit(spans, 0, n, TokenKind::String);
        state.mode = Mode::Fence;
        return;
    }
    if i >= n {
        return;
    }
    if buf[i] == b'#' {
        emit(spans, i, n, TokenKind::Keyword);
        return;
    }
    if buf[i] == b'>' {
        emit(spans, i, n, TokenKind::Comment);
        return;
    }
    if (buf[i] == b'-' || buf[i] == b'*' || buf[i] == b'+') && i + 1 < n && is_space(buf[i + 1]) {
        emit(spans, i, i + 1, TokenKind::Keyword);
        i += 2;
    } else if is_digit(buf[i]) {
        let mut j = i;
        while j < n && is_digit(buf[j]) {
            j += 1;
        }
        if j < n && buf[j] == b'.' {
            emit(spans, i, j + 1, TokenKind::Keyword);
            i = j + 1;
        }
    }
    while i < n {
        let c = buf[i];
        if c == b'`' {
            let mut j = i + 1;
            while j < n && buf[j] != b'`' {
                j += 1;
            }
            let end = n.min(j + 1);
            emit(spans, i, end, TokenKind::String);
            i = end;
        } else if c == b']' && i + 1 < n && buf[i + 1] == b'(' {
            let mut j = i + 2;
            while j < n && buf[j] != b')' {
                j += 1;
            }
            emit(spans, i + 2, j, TokenKind::Function);
            i = n.min(j + 1);
        } else if c == b'*' && i + 1 < n && buf[i + 1] == b'*' {
            if let Some(close) = find(buf, i + 2, b"**") {
                emit(spans, i, close + 2, TokenKind::Constant);
                i = close + 2;
            } else {
                i += 2;
            }
        } else {
            i += 1;
        }
    }
}

// MARK: key = value

fn scan_key_value(buf: &[u8], spans: &mut Vec<SyntaxSpan>) {
    let n = buf.len();
    let mut i = 0;
    while i < n && is_space(buf[i]) {
        i += 1;
    }
    if i >= n {
        return;
    }
    let c = buf[i];
    if c == b'#' || c == b';' || c == b'!' {
        emit(spans, i, n, TokenKind::Comment);
        return;
    }
    if c == b'[' {
        let mut j = i;
        while j < n && buf[j] != b']' {
            j += 1;
        }
        emit(spans, i, n.min(j + 1), TokenKind::Tag);
        scan_scalars(buf, n.min(j + 1), spans);
        return;
    }
    let mut j = i;
    while j < n && buf[j] != b'=' && buf[j] != b':' {
        j += 1;
    }
    if j < n {
        let mut key_end = j;
        while key_end > i && is_space(buf[key_end - 1]) {
            key_end -= 1;
        }
        emit(spans, i, key_end, TokenKind::Keyword);
        scan_scalars(buf, j + 1, spans);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(line: &str, lang: &LanguageSpec) -> Vec<(String, TokenKind)> {
        let mut state = State::default();
        let mut spans = Vec::new();
        scan(line, lang, &mut state, &mut spans);
        spans
            .iter()
            .map(|s| (line[s.start as usize..(s.start + s.length) as usize].to_string(), s.kind))
            .collect()
    }

    #[test]
    fn kotlin_line() {
        let k = kinds("fun greet(name: String) = \"hi $name\" // done", &KOTLIN);
        assert_eq!(k[0], ("fun".into(), TokenKind::Keyword));
        assert_eq!(k[1], ("greet".into(), TokenKind::Function));
        assert!(k.iter().any(|(t, kind)| t == "\"hi $name\"" && *kind == TokenKind::String));
        assert_eq!(k.last().unwrap(), &("// done".into(), TokenKind::Comment));
    }

    #[test]
    fn java_declaration_by_shape() {
        let k = kinds("public static int countLines(String s) {", &JAVA);
        assert!(k.contains(&("countLines".into(), TokenKind::Function)));
        assert!(k.contains(&("int".into(), TokenKind::Keyword)));
    }

    #[test]
    fn block_comment_carries_over() {
        let lines = ["/* start", "still comment", "end */ val x = 1"];
        let spans = highlight(&lines, &KOTLIN);
        assert_eq!(spans[1][0].kind, TokenKind::Comment);
        assert_eq!(spans[2][0], SyntaxSpan { start: 0, length: 6, kind: TokenKind::Comment });
    }

    #[test]
    fn non_ascii_spans_are_bytes() {
        let k = kinds("val 名字 = \"中文\"", &KOTLIN);
        assert!(k.contains(&("\"中文\"".into(), TokenKind::String)));
    }

    #[test]
    fn rehighlight_follows_state_changes() {
        let mut lines = vec!["val a = 1".to_string(), "val b = 2".to_string(), "val c = 3".to_string()];
        let (mut spans, mut states) = highlight_tracking(&lines, &KOTLIN);
        lines[0] = "/* open".to_string();
        let range = rehighlight(&lines, &KOTLIN, 0, 1, &mut spans, &mut states);
        assert_eq!(range, 0..3);
        assert_eq!(spans[2][0].kind, TokenKind::Comment);
    }

    #[test]
    fn detects_languages() {
        assert_eq!(detect("app/src/Main.kt").name, "Kotlin");
        assert_eq!(detect("build.gradle").name, "Groovy");
        assert_eq!(detect("Dockerfile.dev").name, "Shell");
        assert_eq!(detect("setup.ps1").name, "PowerShell");
        assert_eq!(detect("README").name, "Text");
    }

    #[test]
    fn xml_attributes() {
        let k = kinds("<TextView android:id=\"@+id/x\"/>", &XML);
        assert_eq!(k[0], ("<TextView".into(), TokenKind::Tag));
        assert!(k.contains(&("android:".into(), TokenKind::Constant)));
        assert!(k.contains(&("id".into(), TokenKind::Attribute)));
    }
}
