import XCTest
@testable import DifKit

final class PTYTests: XCTestCase {
    private let size = winsize(ws_row: 24, ws_col: 80, ws_xpixel: 0, ws_ypixel: 0)

    private func spawn(_ script: String) throws -> PTYProcess {
        try XCTUnwrap(PTYProcess.spawn(executable: "/bin/sh", arguments: ["sh", "-c", script],
                                       environment: ["PATH=/usr/bin:/bin", "TERM=xterm-256color"],
                                       directory: NSTemporaryDirectory(), size: size))
    }

    /// Output is handed over on the main queue, so waiting means letting the run loop turn.
    private func spin(timeout: TimeInterval = 10, until done: () -> Bool) {
        let deadline = Date().addingTimeInterval(timeout)
        while !done(), Date() < deadline {
            RunLoop.main.run(mode: .default, before: Date().addingTimeInterval(0.01))
        }
    }

    /// Waits for the child and returns its exit status, or nil if it was killed or never finished.
    private func exitStatus(of pid: pid_t) -> Int32? {
        var status: Int32 = 0
        var reaped = false
        spin { reaped = waitpid(pid, &status, WNOHANG) == pid; return reaped }
        guard reaped, status & 0x7F == 0 else { return nil }
        return (status >> 8) & 0xFF
    }

    func testTheProgramRunsOnItsOwnTerminalOfTheGivenSize() throws {
        let pty = try spawn("tty; stty size; pwd; printf done")
        var output = [UInt8]()
        pty.onData = { output.append(contentsOf: $0) }
        spin { String(decoding: output, as: UTF8.self).contains("done") }
        XCTAssertEqual(exitStatus(of: pty.pid), 0)
        let lines = String(decoding: output, as: UTF8.self).components(separatedBy: "\r\n")
        XCTAssertTrue(lines.first?.hasPrefix("/dev/ttys") ?? false, "\(lines)")
        XCTAssertEqual(lines.dropFirst().first, "24 80")
        XCTAssertEqual(lines.dropFirst(2).first.map { URL(fileURLWithPath: $0).resolvingSymlinksInPath().path },
                       URL(fileURLWithPath: NSTemporaryDirectory()).resolvingSymlinksInPath().path)
        pty.close()
    }

    func testTypedInputReachesTheProgramAndResizingIsSeenByIt() throws {
        let pty = try spawn("read line; echo \"got:$line\"; stty size")
        var output = [UInt8]()
        pty.onData = { output.append(contentsOf: $0) }
        pty.resize(winsize(ws_row: 40, ws_col: 132, ws_xpixel: 0, ws_ypixel: 0))
        pty.write(Array("ping\n".utf8)[...])
        spin { String(decoding: output, as: UTF8.self).contains("40 132") }
        let text = String(decoding: output, as: UTF8.self)
        XCTAssertTrue(text.contains("got:ping"), text)
        XCTAssertTrue(text.contains("40 132"), text)
        XCTAssertEqual(exitStatus(of: pty.pid), 0)
        pty.close()
    }

    func testAFloodOfOutputArrivesWholeAndInOrder() throws {
        let last = 200_000
        let pty = try spawn("seq 1 \(last)")
        var count = 0
        var largest = 0
        var tail = [UInt8]()
        pty.onData = { bytes in
            count += bytes.count
            largest = max(largest, bytes.count)
            tail = Array((tail + bytes).suffix(32))
        }
        // Each number is followed by CR LF, as the terminal's line discipline writes a newline.
        let expected = (1...last).reduce(0) { $0 + String($1).count + 2 }
        spin(timeout: 30) { count >= expected }
        XCTAssertEqual(count, expected)
        XCTAssertTrue(String(decoding: tail, as: UTF8.self).hasSuffix("\r\n199999\r\n200000\r\n"))
        // Reading pauses while the main thread is behind, so no batch grows past the bound.
        XCTAssertLessThanOrEqual(largest, PTYProcess.highWater + PTYProcess.readSize)
        XCTAssertEqual(exitStatus(of: pty.pid), 0)
        pty.close()
    }

    func testAProgramThatCannotBeRunExitsWith127() throws {
        let pty = try XCTUnwrap(PTYProcess.spawn(executable: "/nonexistent/shell", arguments: ["-shell"],
                                                 environment: [], directory: "/", size: size))
        XCTAssertEqual(exitStatus(of: pty.pid), 127)
        pty.close()
    }

    func testTheProgramInheritsNothingTheAppHasOpen() throws {
        // A pipe as `Process` makes them: not close-on-exec. If the child kept the writing end,
        // the reading end would never see end-of-file while the child lives.
        var ends: [Int32] = [0, 0]
        XCTAssertEqual(pipe(&ends), 0)
        defer { Darwin.close(ends[0]) }
        let pty = try spawn("sleep 30")
        Darwin.close(ends[1])
        _ = fcntl(ends[0], F_SETFL, O_NONBLOCK)
        // The child closes its copy a moment after the fork, before it becomes `sh`.
        var byte: UInt8 = 0
        var result = -1
        spin(timeout: 5) { result = read(ends[0], &byte, 1); return result == 0 }
        XCTAssertEqual(result, 0, "end-of-file expected; errno \(errno)")
        // Closing the terminal hangs up on the program.
        XCTAssertNotNil(pty.foregroundGroup)
        pty.close()
        XCTAssertNil(pty.foregroundGroup)
        XCTAssertNil(exitStatus(of: pty.pid), "sleep should have been killed by the hang-up")
    }

    func testClosingStopsDelivery() throws {
        let pty = try spawn("sleep 0.2; echo late; sleep 30")
        var output = [UInt8]()
        pty.onData = { output.append(contentsOf: $0) }
        pty.close()
        pty.close() // harmless twice
        pty.write(Array("ignored\n".utf8)[...])
        spin(timeout: 0.6) { false }
        XCTAssertTrue(output.isEmpty)
        kill(pty.pid, SIGKILL)
        XCTAssertNil(exitStatus(of: pty.pid))
    }
}
