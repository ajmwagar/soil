// Minimal WASI 0.2 shims for a Worker. These satisfy the component boundary;
// they deliberately do not pretend to implement terminal, clock, or stream I/O.
// A Spore that actually calls one of these needs a real shim.
class OutputStream {
  checkWrite() { return 65536n; }
  write(bytes) { return BigInt(bytes.length ?? 0); }
  blockingFlush() {}
  subscribe() { return new Pollable(); }
  [Symbol.dispose]() {}
}

class InputStream {
  read() { return new Uint8Array(); }
  blockingRead() { return new Uint8Array(); }
  subscribe() { return new Pollable(); }
  [Symbol.dispose]() {}
}

class Pollable {
  block() {}
  ready() { return true; }
  [Symbol.dispose]() {}
}

class WasiError {
  constructor(message) { this.message = message ?? "stub error"; }
}

const stdoutStream = new OutputStream();
const stderrStream = new OutputStream();
const stdinStream = new InputStream();

export const getEnvironment = () => [];
export const exit = (status) => {
  throw new WasiError(`guest exited: ${JSON.stringify(status)}`);
};
export const getStdout = () => stdoutStream;
export const getStderr = () => stderrStream;
export const getStdin = () => stdinStream;
export class TerminalInput {}
export class TerminalOutput {}
export const getTerminalStdin = () => undefined;
export const getTerminalStdout = () => undefined;
export const getTerminalStderr = () => undefined;
export const subscribeDuration = () => new Pollable();
export const Error = WasiError;
export { InputStream, OutputStream, Pollable };
export const poll = () => [];
