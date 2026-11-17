//! The few CoreFoundation, CoreGraphics, CoreText and ImageIO calls the test harness needs.
#![allow(non_upper_case_globals, non_snake_case, dead_code)]

use std::ffi::c_void;

pub type CFTypeRef = *const c_void;
pub type CFIndex = isize;
pub type CGFloat = f64;
pub type Boolean = u8;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CGPoint {
    pub x: CGFloat,
    pub y: CGFloat,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CGSize {
    pub width: CGFloat,
    pub height: CGFloat,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CGRect {
    pub origin: CGPoint,
    pub size: CGSize,
}

impl CGRect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        CGRect { origin: CGPoint { x, y }, size: CGSize { width: w, height: h } }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CGAffineTransform {
    pub a: CGFloat,
    pub b: CGFloat,
    pub c: CGFloat,
    pub d: CGFloat,
    pub tx: CGFloat,
    pub ty: CGFloat,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CFRange {
    pub location: CFIndex,
    pub length: CFIndex,
}

#[repr(C)]
pub struct CTParagraphStyleSetting {
    pub spec: u32,
    pub value_size: usize,
    pub value: *const c_void,
}

pub const kCFStringEncodingUTF8: u32 = 0x0800_0100;
pub const kCGImageAlphaPremultipliedLast: u32 = 1;
pub const kCGBitmapByteOrderDefault: u32 = 0;
pub const kCTParagraphStyleSpecifierTabStops: u32 = 4;
pub const kCTParagraphStyleSpecifierDefaultTabInterval: u32 = 5;
pub const kCTLineTruncationEnd: u32 = 1;
pub const kCTLineTruncationMiddle: u32 = 2;
pub const kCTFontUIFontSystem: u32 = 2;
pub const kCFNumberFloat64Type: CFIndex = 13;
pub const kCGLineCapRound: i32 = 1;
pub const kCGPathFill: i32 = 0;
pub const kCGPathEOFill: i32 = 1;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    pub static kCFTypeDictionaryKeyCallBacks: c_void;
    pub static kCFTypeDictionaryValueCallBacks: c_void;
    pub static kCFTypeArrayCallBacks: c_void;
    pub static kCFBooleanTrue: CFTypeRef;
    pub fn CFRelease(cf: CFTypeRef);
    pub fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    pub fn CFStringCreateWithBytes(
        alloc: CFTypeRef,
        bytes: *const u8,
        len: CFIndex,
        encoding: u32,
        external: Boolean,
    ) -> CFTypeRef;
    pub fn CFStringGetLength(s: CFTypeRef) -> CFIndex;
    pub fn CFAttributedStringCreateMutable(alloc: CFTypeRef, max: CFIndex) -> CFTypeRef;
    pub fn CFAttributedStringReplaceString(s: CFTypeRef, range: CFRange, replacement: CFTypeRef);
    pub fn CFAttributedStringSetAttribute(s: CFTypeRef, range: CFRange, name: CFTypeRef, value: CFTypeRef);
    pub fn CFAttributedStringBeginEditing(s: CFTypeRef);
    pub fn CFAttributedStringEndEditing(s: CFTypeRef);
    pub fn CFArrayGetCount(a: CFTypeRef) -> CFIndex;
    pub fn CFArrayGetValueAtIndex(a: CFTypeRef, i: CFIndex) -> CFTypeRef;
    pub fn CFArrayCreate(alloc: CFTypeRef, values: *const CFTypeRef, n: CFIndex, cb: *const c_void) -> CFTypeRef;
    pub fn CFDictionaryCreate(
        alloc: CFTypeRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        n: CFIndex,
        kcb: *const c_void,
        vcb: *const c_void,
    ) -> CFTypeRef;
    pub fn CFDictionaryGetValue(d: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    pub fn CFNumberCreate(alloc: CFTypeRef, kind: CFIndex, value: *const c_void) -> CFTypeRef;
    pub fn CFDataCreate(alloc: CFTypeRef, bytes: *const u8, len: CFIndex) -> CFTypeRef;
    pub fn CFURLCreateFromFileSystemRepresentation(
        alloc: CFTypeRef,
        buf: *const u8,
        len: CFIndex,
        is_dir: Boolean,
    ) -> CFTypeRef;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    pub fn CGColorSpaceCreateDeviceRGB() -> CFTypeRef;
    pub fn CGColorSpaceCreateWithName(name: CFTypeRef) -> CFTypeRef;
    pub static kCGColorSpaceSRGB: CFTypeRef;
    pub fn CGBitmapContextCreate(
        data: *mut c_void,
        w: usize,
        h: usize,
        bpc: usize,
        bpr: usize,
        space: CFTypeRef,
        info: u32,
    ) -> CFTypeRef;
    pub fn CGBitmapContextCreateImage(ctx: CFTypeRef) -> CFTypeRef;
    pub fn CGColorCreateSRGB(r: CGFloat, g: CGFloat, b: CGFloat, a: CGFloat) -> CFTypeRef;
    pub fn CGContextSetRGBFillColor(ctx: CFTypeRef, r: CGFloat, g: CGFloat, b: CGFloat, a: CGFloat);
    pub fn CGContextSetRGBStrokeColor(ctx: CFTypeRef, r: CGFloat, g: CGFloat, b: CGFloat, a: CGFloat);
    pub fn CGContextFillRect(ctx: CFTypeRef, r: CGRect);
    pub fn CGContextStrokeRectWithWidth(ctx: CFTypeRef, r: CGRect, w: CGFloat);
    pub fn CGContextSetLineWidth(ctx: CFTypeRef, w: CGFloat);
    pub fn CGContextSetLineCap(ctx: CFTypeRef, cap: i32);
    pub fn CGContextSetLineDash(ctx: CFTypeRef, phase: CGFloat, lengths: *const CGFloat, count: usize);
    pub fn CGContextBeginPath(ctx: CFTypeRef);
    pub fn CGContextMoveToPoint(ctx: CFTypeRef, x: CGFloat, y: CGFloat);
    pub fn CGContextAddLineToPoint(ctx: CFTypeRef, x: CGFloat, y: CGFloat);
    pub fn CGContextAddCurveToPoint(ctx: CFTypeRef, a: CGFloat, b: CGFloat, c: CGFloat, d: CGFloat, x: CGFloat, y: CGFloat);
    pub fn CGContextAddQuadCurveToPoint(ctx: CFTypeRef, a: CGFloat, b: CGFloat, x: CGFloat, y: CGFloat);
    pub fn CGContextClosePath(ctx: CFTypeRef);
    pub fn CGContextAddPath(ctx: CFTypeRef, path: CFTypeRef);
    pub fn CGContextDrawPath(ctx: CFTypeRef, mode: i32);
    pub fn CGContextStrokePath(ctx: CFTypeRef);
    pub fn CGContextAddEllipseInRect(ctx: CFTypeRef, r: CGRect);
    pub fn CGContextFillPath(ctx: CFTypeRef);
    pub fn CGPathCreateWithRoundedRect(r: CGRect, rx: CGFloat, ry: CGFloat, t: *const c_void) -> CFTypeRef;
    pub fn CGContextSaveGState(ctx: CFTypeRef);
    pub fn CGContextRestoreGState(ctx: CFTypeRef);
    pub fn CGContextClipToRect(ctx: CFTypeRef, r: CGRect);
    pub fn CGContextTranslateCTM(ctx: CFTypeRef, x: CGFloat, y: CGFloat);
    pub fn CGContextScaleCTM(ctx: CFTypeRef, x: CGFloat, y: CGFloat);
    pub fn CGContextSetTextMatrix(ctx: CFTypeRef, t: CGAffineTransform);
    pub fn CGContextSetTextPosition(ctx: CFTypeRef, x: CGFloat, y: CGFloat);
    pub fn CGContextSetShadowWithColor(ctx: CFTypeRef, offset: CGSize, blur: CGFloat, color: CFTypeRef);
    pub fn CGContextSetShouldSmoothFonts(ctx: CFTypeRef, v: bool);
    pub fn CGContextSetAllowsFontSmoothing(ctx: CFTypeRef, v: bool);
    pub fn CGContextSetShouldAntialias(ctx: CFTypeRef, v: bool);
    pub fn CGContextDrawImage(ctx: CFTypeRef, r: CGRect, image: CFTypeRef);
    pub fn CGImageGetWidth(image: CFTypeRef) -> usize;
    pub fn CGImageGetHeight(image: CFTypeRef) -> usize;
}

#[link(name = "CoreText", kind = "framework")]
extern "C" {
    pub static kCTFontAttributeName: CFTypeRef;
    pub static kCTForegroundColorAttributeName: CFTypeRef;
    pub static kCTForegroundColorFromContextAttributeName: CFTypeRef;
    pub static kCTParagraphStyleAttributeName: CFTypeRef;
    pub static kCTFontTraitsAttribute: CFTypeRef;
    pub static kCTFontWeightTrait: CFTypeRef;
    pub fn CTFontCreateWithName(name: CFTypeRef, size: CGFloat, matrix: *const c_void) -> CFTypeRef;
    pub fn CTFontCreateUIFontForLanguage(kind: u32, size: CGFloat, lang: CFTypeRef) -> CFTypeRef;
    pub fn CTFontCopyFamilyName(font: CFTypeRef) -> CFTypeRef;
    pub fn CTFontGetSymbolicTraits(font: CFTypeRef) -> u32;
    pub fn CTFontManagerCopyAvailableFontFamilyNames() -> CFTypeRef;
    pub fn CTFontDescriptorCreateWithAttributes(attrs: CFTypeRef) -> CFTypeRef;
    pub fn CTFontCreateCopyWithAttributes(font: CFTypeRef, size: CGFloat, matrix: *const c_void, desc: CFTypeRef) -> CFTypeRef;
    pub fn CTFontCreateCopyWithSymbolicTraits(
        font: CFTypeRef,
        size: CGFloat,
        matrix: *const c_void,
        value: u32,
        mask: u32,
    ) -> CFTypeRef;
    pub fn CTFontGetAscent(font: CFTypeRef) -> CGFloat;
    pub fn CTFontGetDescent(font: CFTypeRef) -> CGFloat;
    pub fn CTFontGetLeading(font: CFTypeRef) -> CGFloat;
    pub fn CTParagraphStyleCreate(settings: *const CTParagraphStyleSetting, n: usize) -> CFTypeRef;
    pub fn CTLineCreateWithAttributedString(s: CFTypeRef) -> CFTypeRef;
    pub fn CTLineCreateTruncatedLine(line: CFTypeRef, width: f64, kind: u32, token: CFTypeRef) -> CFTypeRef;
    pub fn CTLineGetTypographicBounds(line: CFTypeRef, a: *mut CGFloat, d: *mut CGFloat, l: *mut CGFloat) -> f64;
    pub fn CTLineGetStringIndexForPosition(line: CFTypeRef, p: CGPoint) -> CFIndex;
    pub fn CTLineGetOffsetForStringIndex(line: CFTypeRef, i: CFIndex, secondary: *mut CGFloat) -> CGFloat;
    pub fn CTLineGetStringRange(line: CFTypeRef) -> CFRange;
    pub fn CTLineDraw(line: CFTypeRef, ctx: CFTypeRef);
    pub fn CTLineGetGlyphRuns(line: CFTypeRef) -> CFTypeRef;
    pub fn CTRunGetAttributes(run: CFTypeRef) -> CFTypeRef;
    pub fn CTRunDraw(run: CFTypeRef, ctx: CFTypeRef, range: CFRange);
    pub fn CTFramesetterCreateWithAttributedString(s: CFTypeRef) -> CFTypeRef;
    pub fn CTTypesetterCreateWithAttributedString(s: CFTypeRef) -> CFTypeRef;
    pub fn CTTypesetterSuggestLineBreak(ts: CFTypeRef, start: CFIndex, width: f64) -> CFIndex;
    pub fn CTTypesetterCreateLine(ts: CFTypeRef, range: CFRange) -> CFTypeRef;
}

#[link(name = "ImageIO", kind = "framework")]
extern "C" {
    pub fn CGImageDestinationCreateWithURL(url: CFTypeRef, kind: CFTypeRef, count: usize, opts: CFTypeRef) -> CFTypeRef;
    pub fn CGImageDestinationAddImage(dest: CFTypeRef, image: CFTypeRef, props: CFTypeRef);
    pub fn CGImageDestinationFinalize(dest: CFTypeRef) -> bool;
    pub fn CGImageSourceCreateWithData(data: CFTypeRef, opts: CFTypeRef) -> CFTypeRef;
    pub fn CGImageSourceCreateImageAtIndex(src: CFTypeRef, i: usize, opts: CFTypeRef) -> CFTypeRef;
}

/// A retained CoreFoundation object, released when dropped.
pub struct Cf(pub CFTypeRef);

impl Cf {
    pub fn ptr(&self) -> CFTypeRef {
        self.0
    }
}

impl Drop for Cf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) }
        }
    }
}

impl Clone for Cf {
    fn clone(&self) -> Self {
        if !self.0.is_null() {
            unsafe { CFRetain(self.0) };
        }
        Cf(self.0)
    }
}

pub fn cfstr(s: &str) -> Cf {
    Cf(unsafe { CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as CFIndex, kCFStringEncodingUTF8, 0) })
}

pub fn cfnum(v: f64) -> Cf {
    Cf(unsafe { CFNumberCreate(std::ptr::null(), kCFNumberFloat64Type, &v as *const f64 as *const c_void) })
}
