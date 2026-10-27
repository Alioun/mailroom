import Darwin
import Foundation

/// A zero core-file limit, soft and hard, before anything sensitive is read.
public func disableCoreDumps() throws {
    var limit = rlimit(rlim_cur: 0, rlim_max: 0)
    guard setrlimit(RLIMIT_CORE, &limit) == 0, getrlimit(RLIMIT_CORE, &limit) == 0,
          limit.rlim_cur == 0, limit.rlim_max == 0 else { throw Denied() }
}

/// The parent hands over anonymous pipes and no arguments. Terminals, files and sockets deny.
public func requirePrivatePipes() throws {
    guard CommandLine.arguments.count == 1 else { throw Denied() }
    for fd in [STDIN_FILENO, STDOUT_FILENO] {
        var info = stat()
        guard isatty(fd) == 0, fstat(fd, &info) == 0, info.st_mode & S_IFMT == S_IFIFO else { throw Denied() }
    }
}

public func readRequest(limit: Int = Limits.envelope) throws -> Data {
    guard let input = try FileHandle.standardInput.read(upToCount: limit + 1), input.count <= limit else { throw Denied() }
    return input
}

public func write(_ output: Data) {
    FileHandle.standardOutput.write(output)
}
