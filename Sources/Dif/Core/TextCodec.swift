import CryptoKit
import Foundation

/// How a file's text sits on disk, so that an edited file can be written back byte for byte the
/// same everywhere it was not edited.
struct TextFormat: Equatable {
    enum Encoding: Equatable { case utf8, utf16LittleEndian, utf16BigEndian, gb18030 }

    var encoding: Encoding = .utf8
    var bom = false
    /// Ends the lines that editing adds: whichever the file uses most.
    var newline = "\n"
    /// The last line ends with a line break.
    var finalNewline = false
}

/// A file's text split into lines exactly as `TextCodec.splitLines` splits it, plus what it takes to write it back.
struct EditableText {
    var lines: [String]
    var format: TextFormat
    /// Per line, whether it ends in "\r\n". Kept only for files that mix line endings; nil when all end alike.
    var crlf: [Bool]?
}

/// Decoding and line-splitting for file contents.
enum TextCodec {
    /// Git's own heuristic: a NUL byte in the first 8000 bytes means binary.
    static func looksBinary(_ data: Data) -> Bool {
        if data.starts(with: [0xFF, 0xFE]) || data.starts(with: [0xFE, 0xFF]) { return false } // UTF-16 BOM
        return data.prefix(8000).contains(0)
    }

    /// Legacy Chinese sources are common enough to try before giving up.
    static let gb18030 = String.Encoding(rawValue: CFStringConvertEncodingToNSStringEncoding(
        CFStringEncoding(CFStringEncodings.GB_18030_2000.rawValue)))

    static func decode(_ data: Data) -> String {
        if data.starts(with: [0xEF, 0xBB, 0xBF]) {
            return String(decoding: data.dropFirst(3), as: UTF8.self)
        }
        if data.starts(with: [0xFF, 0xFE]) || data.starts(with: [0xFE, 0xFF]) {
            if let s = String(data: data, encoding: .utf16) { return s }
        }
        if let s = String(data: data, encoding: .utf8) { return s }
        if let s = String(data: data, encoding: gb18030) { return s }
        return String(decoding: data, as: UTF8.self)
    }

    /// Decodes a file for editing. Nil unless writing the result back with `encode` gives exactly `data`:
    /// bytes no encoding accepts, a carriage return that does not end a line and the like keep a file read-only.
    static func decodeForEditing(_ data: Data) -> EditableText? {
        var format = TextFormat()
        let text: String?
        if data.starts(with: [0xEF, 0xBB, 0xBF]) {
            format.bom = true
            text = String(data: data.dropFirst(3), encoding: .utf8)
        } else if data.starts(with: [0xFF, 0xFE]) {
            format.bom = true
            format.encoding = .utf16LittleEndian
            text = String(data: data.dropFirst(2), encoding: .utf16LittleEndian)
        } else if data.starts(with: [0xFE, 0xFF]) {
            format.bom = true
            format.encoding = .utf16BigEndian
            text = String(data: data.dropFirst(2), encoding: .utf16BigEndian)
        } else if let utf8 = String(data: data, encoding: .utf8) {
            text = utf8
        } else {
            format.encoding = .gb18030
            text = String(data: data, encoding: gb18030)
        }
        guard var text else { return nil }

        var lines: [String] = []
        var endsInCRLF: [Bool] = []
        var crlfCount = 0, lfCount = 0
        var stray = false
        text.withUTF8 { buf in
            lines.reserveCapacity(buf.count / 32 + 1)
            var start = 0
            for i in 0..<buf.count {
                let byte = buf[i]
                if byte == 0x0D {
                    // Only as half of "\r\n": a lone one is a line break git does not see, or a character no editor shows.
                    if i + 1 >= buf.count || buf[i + 1] != 0x0A { stray = true }
                    continue
                }
                guard byte == 0x0A else { continue }
                let crlf = i > start && buf[i - 1] == 0x0D
                if crlf { crlfCount += 1 } else { lfCount += 1 }
                lines.append(String(decoding: UnsafeBufferPointer(rebasing: buf[start..<(crlf ? i - 1 : i)]), as: UTF8.self))
                endsInCRLF.append(crlf)
                start = i + 1
            }
            if start < buf.count {
                lines.append(String(decoding: UnsafeBufferPointer(rebasing: buf[start..<buf.count]), as: UTF8.self))
                endsInCRLF.append(false)
            }
            format.finalNewline = !buf.isEmpty && start == buf.count
        }
        guard !stray else { return nil }
        // An empty file still has a line to type on.
        if lines.isEmpty {
            lines = [""]
            endsInCRLF = [false]
        }
        format.newline = crlfCount > lfCount ? "\r\n" : "\n"
        let result = EditableText(lines: lines, format: format, crlf: crlfCount > 0 && lfCount > 0 ? endsInCRLF : nil)
        guard encode(result.lines, format: format, crlf: result.crlf) == data else { return nil }
        return result
    }

    /// The bytes for `lines` in `format`. Nil only if the text cannot be represented in the file's encoding.
    static func encode(_ lines: [String], format: TextFormat, crlf: [Bool]?) -> Data? {
        var text = ""
        text.reserveCapacity(lines.reduce(0) { $0 + $1.utf8.count + 2 })
        let last = lines.count - 1
        for (index, line) in lines.enumerated() {
            text += line
            guard index < last || format.finalNewline else { continue }
            if let crlf, index < crlf.count {
                text += crlf[index] ? "\r\n" : "\n"
            } else {
                text += format.newline
            }
        }
        var data = Data()
        switch format.encoding {
        case .utf8:
            if format.bom { data.append(contentsOf: [0xEF, 0xBB, 0xBF]) }
            data.append(contentsOf: text.utf8)
        case .utf16LittleEndian:
            if format.bom { data.append(contentsOf: [0xFF, 0xFE]) }
            guard let body = text.data(using: .utf16LittleEndian) else { return nil }
            data.append(body)
        case .utf16BigEndian:
            if format.bom { data.append(contentsOf: [0xFE, 0xFF]) }
            guard let body = text.data(using: .utf16BigEndian) else { return nil }
            data.append(body)
        case .gb18030:
            guard let body = text.data(using: gb18030) else { return nil }
            data.append(body)
        }
        return data
    }

    /// Identifies a file's contents, to tell whether something else rewrote it.
    static func fingerprint(_ data: Data) -> Data {
        Data(SHA256.hash(data: data))
    }

    /// Splits on `\n`, dropping a trailing `\r` from each line. A final newline does not
    /// produce an extra empty line, matching how git counts lines.
    static func splitLines(_ text: String) -> [String] {
        var text = text
        var lines: [String] = []
        text.withUTF8 { buf in
            guard !buf.isEmpty else { return }
            lines.reserveCapacity(buf.count / 32 + 1)
            var start = 0
            var i = 0
            let n = buf.count
            while i < n {
                if buf[i] == 0x0A {
                    var end = i
                    if end > start, buf[end - 1] == 0x0D { end -= 1 }
                    lines.append(String(decoding: UnsafeBufferPointer(rebasing: buf[start..<end]), as: UTF8.self))
                    start = i + 1
                }
                i += 1
            }
            if start < n {
                var end = n
                if end > start, buf[end - 1] == 0x0D { end -= 1 }
                lines.append(String(decoding: UnsafeBufferPointer(rebasing: buf[start..<end]), as: UTF8.self))
            }
        }
        return lines
    }
}

extension Int {
    /// "1.2 MB"-style size.
    var byteSizeString: String {
        ByteCountFormatter.string(fromByteCount: Int64(self), countStyle: .file)
    }
}
