import Foundation

/// Fuzzy file-name matching in the spirit of IntelliJ's "Go to File": characters must appear
/// in order, and matches at word starts, camelCase humps and inside the file name score higher.
enum Fuzzy {
    struct Match {
        var score: Int
        /// Matched character offsets into the candidate's UTF-8 bytes (ASCII-safe for highlighting).
        var positions: [Int]
    }

    @inline(__always) private static func lower(_ c: UInt8) -> UInt8 { (c >= 65 && c <= 90) ? c + 32 : c }
    @inline(__always) private static func isUpper(_ c: UInt8) -> Bool { c >= 65 && c <= 90 }
    @inline(__always) private static func isSeparator(_ c: UInt8) -> Bool {
        c == 47 || c == 95 || c == 45 || c == 46 || c == 32
    }

    /// Scores `candidate` against a lowercased query. Returns nil when the query is not a subsequence.
    static func match(query: [UInt8], candidate: String, wantPositions: Bool) -> Match? {
        var candidate = candidate
        return candidate.withUTF8 { buf -> Match? in
            let n = buf.count, m = query.count
            guard m > 0, n >= m else { return nil }

            // Cheap subsequence rejection before any scoring.
            var qi = 0
            for i in 0..<n where lower(buf[i]) == query[qi] {
                qi += 1
                if qi == m { break }
            }
            guard qi == m else { return nil }

            var nameStart = 0
            for i in stride(from: n - 1, through: 0, by: -1) where buf[i] == 47 { nameStart = i + 1; break }

            // Prefer matching entirely inside the file name; fall back to the whole path.
            if let inName = score(buf, from: nameStart, query, wantPositions: wantPositions) {
                var result = inName
                result.score += 1000 - min(400, n - nameStart) // shorter names win ties
                return result
            }
            guard var whole = score(buf, from: 0, query, wantPositions: wantPositions) else { return nil }
            whole.score -= min(300, n)
            return whole
        }
    }

    /// Greedy scorer: for each query character take the best-scoring next position within a small lookahead.
    private static func score(_ buf: UnsafeBufferPointer<UInt8>, from: Int, _ query: [UInt8], wantPositions: Bool) -> Match? {
        let n = buf.count, m = query.count
        guard n - from >= m else { return nil }
        // dp[j] = best score for matching query[0...qi] with query[qi] at position j.
        let width = n - from
        let negative = Int.min / 4
        var previous = [Int](repeating: negative, count: width)
        var current = [Int](repeating: negative, count: width)
        var backtrack: [[Int32]] = wantPositions ? Array(repeating: [Int32](repeating: -1, count: width), count: m) : []

        func bonus(at j: Int) -> Int {
            let i = from + j
            if i == from { return 40 }                       // start of the name
            let prev = buf[i - 1]
            if isSeparator(prev) { return 30 }               // after / _ - .
            if isUpper(buf[i]) && !isUpper(prev) { return 28 } // camelCase hump
            return 0
        }

        for qi in 0..<m {
            var bestPrev = negative   // best of previous[0..<j]
            var bestPrevIndex = -1
            for j in 0..<width {
                current[j] = negative
                if j >= qi, lower(buf[from + j]) == query[qi] {
                    if qi == 0 {
                        current[j] = 10 + bonus(at: j) - min(j, 20)
                    } else {
                        // Either continue directly from the previous character (consecutive bonus) or jump.
                        var best = negative
                        var bestFrom = -1
                        if j > 0, previous[j - 1] > negative {
                            best = previous[j - 1] + 10 + 35
                            bestFrom = j - 1
                        }
                        if bestPrev > negative {
                            let jump = bestPrev + 10 + bonus(at: j) - 2
                            if jump > best { best = jump; bestFrom = bestPrevIndex }
                        }
                        if bestFrom >= 0 {
                            current[j] = best
                            if wantPositions { backtrack[qi][j] = Int32(bestFrom) }
                        }
                    }
                }
                // Make previous[j-1] available to the *next* j as a non-consecutive jump source.
                if j > 0, qi > 0, previous[j - 1] > bestPrev { bestPrev = previous[j - 1]; bestPrevIndex = j - 1 }
            }
            swap(&previous, &current)
        }

        var best = negative, bestIndex = -1
        for j in 0..<width where previous[j] > best { best = previous[j]; bestIndex = j }
        guard bestIndex >= 0 else { return nil }

        var positions: [Int] = []
        if wantPositions {
            var j = bestIndex
            var qi = m - 1
            while qi >= 0, j >= 0 {
                positions.append(from + j)
                j = Int(backtrack[qi][j])
                qi -= 1
            }
            positions.reverse()
        }
        return Match(score: best, positions: positions)
    }

    struct Ranked {
        var path: String
        var score: Int
        var positions: [Int]
    }

    /// Top `limit` matches for `query` among `paths`, best first.
    static func rank(query: String, in paths: [String], limit: Int, isCancelled: () -> Bool = { false }) -> [Ranked] {
        let q = Array(query.lowercased().utf8).filter { $0 != 32 }
        guard !q.isEmpty else { return [] }
        var scored: [(Int, Int)] = [] // (score, index)
        scored.reserveCapacity(1024)
        for (index, path) in paths.enumerated() {
            if index & 0x3FF == 0, isCancelled() { return [] }
            if let m = match(query: q, candidate: path, wantPositions: false) {
                scored.append((m.score, index))
            }
        }
        scored.sort { $0.0 != $1.0 ? $0.0 > $1.0 : paths[$0.1].utf8.count < paths[$1.1].utf8.count }
        return scored.prefix(limit).compactMap { entry in
            let path = paths[entry.1]
            guard let m = match(query: q, candidate: path, wantPositions: true) else { return nil }
            return Ranked(path: path, score: m.score, positions: m.positions)
        }
    }
}
