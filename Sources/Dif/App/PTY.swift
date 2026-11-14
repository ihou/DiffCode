import Darwin
import Foundation

/// A program running on a pseudo-terminal, and the plumbing between it and the terminal view.
///
/// SwiftTerm ships its own (`LocalProcess`), but that one reads through `DispatchIO`, which hands over a
/// fresh buffer for every few hundred bytes: a build log scrolling past cost hundreds of megabytes, and a
/// long one gigabytes. Here output is read into one buffer that is reused, and reading pauses while the
/// main thread is behind, so a noisy program is slowed down instead of piling up in memory.
final class PTYProcess: @unchecked Sendable {
    /// Most bytes taken from the terminal in one read.
    static let readSize = 32 * 1024
    /// Unparsed output allowed to wait for the main thread before reading pauses.
    static let highWater = 64 * 1024

    let pid: pid_t
    /// The controlling side of the terminal; -1 once closed. Main thread only.
    private(set) var descriptor: Int32

    /// Output of the program, delivered on the main thread. The slice is only valid during the call.
    var onData: ((ArraySlice<UInt8>) -> Void)?

    private let fd: Int32
    private let readQueue = DispatchQueue(label: "dif.pty.read", qos: .userInitiated)
    private let writeQueue = DispatchQueue(label: "dif.pty.write", qos: .userInitiated)
    private var source: DispatchSourceRead?
    private var scratch = [UInt8](repeating: 0, count: PTYProcess.readSize)

    private let lock = NSLock()
    // Guarded by `lock`:
    private var pending: [UInt8] = []
    private var drainScheduled = false
    private var suspended = false
    private var closed = false
    private var reachedEnd = false

    /// Main thread only: the buffer being parsed, swapped with `pending` so neither is reallocated.
    private var delivering: [UInt8] = []
    private var isDelivering = false

    private init(pid: pid_t, descriptor: Int32) {
        self.pid = pid
        self.descriptor = descriptor
        fd = descriptor
    }

    deinit { close() }

    // MARK: Launch

    /// Starts `executable` on a new terminal of the given size, as the leader of its own session.
    /// `arguments` includes `argv[0]`. Returns nil when no terminal could be created.
    ///
    /// A missing or unrunnable executable is not an error here: the child exits with status 127.
    static func spawn(executable: String, arguments: [String], environment: [String],
                      directory: String, size: winsize) -> PTYProcess? {
        // Everything the child needs is laid out before the fork. Between fork and exec the child is a
        // copy of a multi-threaded app and may only make plain system calls, never allocate.
        let path = strdup(executable)
        let workingDirectory = strdup(directory)
        var argv: [UnsafeMutablePointer<CChar>?] = arguments.map { strdup($0) } + [nil]
        var envp: [UnsafeMutablePointer<CChar>?] = environment.map { strdup($0) } + [nil]
        defer {
            free(path)
            free(workingDirectory)
            argv.forEach { free($0) }
            envp.forEach { free($0) }
        }
        let descriptorLimit = min(getdtablesize(), 10_240)
        var size = size
        var master: Int32 = -1
        let pid = argv.withUnsafeMutableBufferPointer { argv in
            envp.withUnsafeMutableBufferPointer { envp -> pid_t in
                let pid = forkpty(&master, nil, nil, &size)
                guard pid == 0 else { return pid }
                // In the child. The terminal is already its standard input, output and error.
                var settings = termios()
                if tcgetattr(0, &settings) == 0 {
                    // Lets the kernel's own line editing erase a whole CJK character, not one byte of it.
                    settings.c_iflag |= tcflag_t(IUTF8)
                    _ = tcsetattr(0, TCSANOW, &settings)
                }
                // Nothing else the app has open belongs in the shell: a pipe to a `git` that happens to be
                // running would otherwise stay open for as long as the shell lives.
                var other: Int32 = 3
                while other < descriptorLimit {
                    Darwin.close(other)
                    other += 1
                }
                var number: Int32 = 1
                while number < NSIG {
                    signal(number, SIG_DFL)
                    number += 1
                }
                var none = sigset_t()
                sigemptyset(&none)
                sigprocmask(SIG_SETMASK, &none, nil)
                _ = chdir(workingDirectory)
                execve(path, argv.baseAddress, envp.baseAddress)
                _exit(127)
            }
        }
        guard pid > 0, master >= 0 else { return nil }
        // Later children must not inherit this terminal, and a full terminal must never block the app.
        _ = fcntl(master, F_SETFD, FD_CLOEXEC)
        _ = fcntl(master, F_SETFL, fcntl(master, F_GETFL) | O_NONBLOCK)
        let process = PTYProcess(pid: pid, descriptor: master)
        process.startReading()
        return process
    }

    // MARK: Output

    private func startReading() {
        let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: readQueue)
        source.setEventHandler { [weak self] in self?.readAvailable() }
        // The descriptor is closed only after the last read and the last write that could use it.
        source.setCancelHandler { [fd, writeQueue] in writeQueue.async { Darwin.close(fd) } }
        self.source = source
        source.activate()
    }

    /// On the read queue: takes what the terminal has and hands it to the main thread.
    private func readAvailable() {
        let count = scratch.withUnsafeMutableBytes { Darwin.read(fd, $0.baseAddress, $0.count) }
        if count < 0, errno == EAGAIN || errno == EINTR { return }
        lock.lock()
        guard !closed else {
            lock.unlock()
            return
        }
        if count > 0 {
            pending.append(contentsOf: scratch[0..<count])
            if pending.count >= Self.highWater { pause() }
        } else {
            // Every program on the terminal has gone. Stop listening; the owner closes when the shell is reaped.
            reachedEnd = true
            pause()
        }
        let schedule = !drainScheduled && !pending.isEmpty
        if schedule { drainScheduled = true }
        lock.unlock()
        if schedule { DispatchQueue.main.async { [weak self] in self?.drain() } }
    }

    /// With `lock` held.
    private func pause() {
        guard !suspended else { return }
        suspended = true
        source?.suspend()
    }

    /// On the main thread: parses everything read since the last time, in one go.
    private func drain() {
        // Parsing can, in principle, spin the run loop; a second batch must not start inside the first.
        guard !isDelivering else {
            DispatchQueue.main.async { [weak self] in self?.drain() }
            return
        }
        isDelivering = true
        defer { isDelivering = false }
        lock.lock()
        swap(&pending, &delivering)
        drainScheduled = false
        let deliver = !closed
        if suspended, !reachedEnd, !closed {
            suspended = false
            source?.resume() // reading carries on while this batch is parsed
        }
        lock.unlock()
        if deliver, !delivering.isEmpty { onData?(delivering[...]) }
        delivering.removeAll(keepingCapacity: true)
    }

    // MARK: Input

    /// Sends keystrokes or pasted text to the program. Never blocks, however slowly the program reads.
    func write(_ bytes: ArraySlice<UInt8>) {
        guard descriptor >= 0, !bytes.isEmpty else { return }
        let data = Array(bytes)
        writeQueue.async { [self] in
            data.withUnsafeBytes { raw in
                var offset = 0
                while offset < raw.count {
                    if isClosed { return }
                    let written = Darwin.write(fd, raw.baseAddress! + offset, raw.count - offset)
                    if written > 0 {
                        offset += written
                    } else if written < 0, errno == EAGAIN {
                        usleep(2_000) // the program is not reading; give it a moment
                    } else if written < 0, errno == EINTR {
                        continue
                    } else {
                        return // nobody is on the other end any more
                    }
                }
            }
        }
    }

    private var isClosed: Bool {
        lock.lock()
        defer { lock.unlock() }
        return closed
    }

    // MARK: Terminal state (main thread)

    func resize(_ size: winsize) {
        guard descriptor >= 0 else { return }
        var size = size
        _ = ioctl(descriptor, TIOCSWINSZ, &size)
    }

    /// The process group that owns the terminal right now, or nil when that cannot be told.
    var foregroundGroup: pid_t? {
        guard descriptor >= 0 else { return nil }
        let group = tcgetpgrp(descriptor)
        return group > 0 ? group : nil
    }

    /// Stops reading and gives the terminal up. Whatever still has it open is hung up on by the kernel.
    func close() {
        lock.lock()
        guard !closed else {
            lock.unlock()
            return
        }
        closed = true
        // A suspended source never runs its cancel handler, so it would never close the descriptor.
        if suspended {
            suspended = false
            source?.resume()
        }
        lock.unlock()
        descriptor = -1
        onData = nil
        source?.cancel()
        source = nil
    }
}
