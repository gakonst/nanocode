import io
import os
import sys
import errno
import _host

if not _host.spawns():
    raise ImportError('this Python has no processes to run a command in')

__all__ = ['Popen', 'PIPE', 'STDOUT', 'DEVNULL', 'call', 'check_call', 'check_output', 'run',
           'getstatusoutput', 'getoutput', 'list2cmdline', 'CompletedProcess', 'SubprocessError',
           'CalledProcessError', 'TimeoutExpired']

PIPE = -1
STDOUT = -2
DEVNULL = -3


class SubprocessError(Exception):
    pass


class CalledProcessError(SubprocessError):
    def __init__(self, returncode, cmd, output=None, stderr=None):
        self.returncode = returncode
        self.cmd = cmd
        self.output = output
        self.stderr = stderr

    def __str__(self):
        return "Command '%s' returned non-zero exit status %d." % (self.cmd, self.returncode)

    @property
    def stdout(self):
        return self.output

    @stdout.setter
    def stdout(self, value):
        self.output = value


class TimeoutExpired(SubprocessError):
    def __init__(self, cmd, timeout, output=None, stderr=None):
        self.cmd = cmd
        self.timeout = timeout
        self.output = output
        self.stderr = stderr

    def __str__(self):
        return "Command '%s' timed out after %s seconds" % (self.cmd, self.timeout)

    @property
    def stdout(self):
        return self.output

    @stdout.setter
    def stdout(self, value):
        self.output = value


class CompletedProcess:
    def __init__(self, args, returncode, stdout=None, stderr=None):
        self.args = args
        self.returncode = returncode
        self.stdout = stdout
        self.stderr = stderr

    def __repr__(self):
        args = ['args={!r}'.format(self.args), 'returncode={!r}'.format(self.returncode)]
        if self.stdout is not None:
            args.append('stdout={!r}'.format(self.stdout))
        if self.stderr is not None:
            args.append('stderr={!r}'.format(self.stderr))
        return '{}({})'.format(type(self).__name__, ', '.join(args))

    def check_returncode(self):
        if self.returncode:
            raise CalledProcessError(self.returncode, self.args, self.stdout, self.stderr)


def list2cmdline(seq):
    result = []
    for arg in map(os.fsdecode, seq):
        if not arg or ' ' in arg or '\t' in arg or '"' in arg:
            arg = '"' + arg.replace('\\', '\\\\').replace('"', '\\"') + '"'
        result.append(arg)
    return ' '.join(result)


class _Feed:
    def __init__(self, process):
        self._process = process
        self._data = io.BytesIO()
        self.closed = False

    def write(self, data):
        if self.closed:
            raise ValueError('write to closed file')
        if isinstance(data, str):
            data = data.encode(self._process.encoding or 'utf-8', self._process.errors or 'strict')
        return self._data.write(data)

    def writelines(self, lines):
        for line in lines:
            self.write(line)

    def flush(self):
        pass

    def writable(self):
        return True

    def fileno(self):
        raise io.UnsupportedOperation('fileno')

    def close(self):
        if not self.closed:
            self.closed = True
            self._process._finish()

    def __enter__(self):
        return self

    def __exit__(self, *error):
        self.close()


class _Lazy:
    def __init__(self, process, which):
        self._process = process
        self._which = which

    def _real(self):
        self._process._finish()
        return self._process._streams[self._which]

    def __getattr__(self, name):
        return getattr(self._real(), name)

    def __iter__(self):
        return iter(self._real())

    def __next__(self):
        return next(self._real())

    def __enter__(self):
        return self

    def __exit__(self, *error):
        self._real().close()


def _drain(source):
    if source is None or source == DEVNULL or source == PIPE:
        return b''
    if isinstance(source, int):
        chunks = []
        while True:
            chunk = os.read(source, io.DEFAULT_BUFFER_SIZE)
            if not chunk:
                return b''.join(chunks)
            chunks.append(chunk)
    data = source.read()
    return data.encode('utf-8', 'surrogateescape') if isinstance(data, str) else data


def _deliver(target, fd, data):
    if target == DEVNULL or target == PIPE or not data:
        return
    if target is None:
        _host.output(fd, data)
        return
    if not isinstance(target, int):
        target.flush()
        target = target.fileno()
    if target in (1, 2):
        _host.output(target, data)
        return
    view = memoryview(data)
    while view:
        view = view[os.write(target, view):]


class Popen:
    _spawned = 0

    def __init__(self, args, bufsize=-1, executable=None, stdin=None, stdout=None, stderr=None,
                 preexec_fn=None, close_fds=True, shell=False, cwd=None, env=None,
                 universal_newlines=None, startupinfo=None, creationflags=0, restore_signals=True,
                 start_new_session=False, pass_fds=(), *, user=None, group=None, extra_groups=None,
                 encoding=None, errors=None, text=None, umask=-1, pipesize=-1, process_group=None):
        if stderr == STDOUT and stdout is None:
            stdout = 1
        self.args = args
        self.returncode = None
        self.encoding = encoding
        self.errors = errors
        self._text = bool(text or universal_newlines or encoding or errors)
        if isinstance(args, (str, bytes)) or hasattr(args, '__fspath__'):
            args = [args]
        argv = [os.fsencode(arg) for arg in args]
        if shell:
            argv = [os.fsencode(executable or '/bin/sh'), b'-c'] + argv
        if not argv:
            raise IndexError('list index out of range')
        self._argv = argv
        self._cwd = os.fsencode(os.path.abspath(os.getcwd() if cwd is None else cwd))
        pairs = (os.environ if env is None else env).items()
        self._env = [os.fsencode(part) for pair in pairs for part in pair]
        self._stdin, self._stdout, self._stderr = stdin, stdout, stderr
        self._ran = False
        self._streams = {}
        Popen._spawned += 1
        self.pid = os.getpid() + Popen._spawned
        self.stdin = _Feed(self) if stdin == PIPE else None
        self.stdout = _Lazy(self, 1) if stdout == PIPE else None
        self.stderr = _Lazy(self, 2) if stderr == PIPE else None
        if stdin != PIPE:
            self._finish()

    def _finish(self):
        if self._ran:
            return
        self._ran = True
        if self.stdin is not None:
            self.stdin.closed = True
            data = self.stdin._data.getvalue()
        else:
            data = _drain(self._stdin)
        for stream in (sys.stdout, sys.stderr):
            try:
                stream.flush()
            except Exception:
                pass
        merged = self._stderr == STDOUT
        try:
            status, out, err = _host.spawn(self._argv, self._env, self._cwd, data, merged)
        except OSError as error:
            self.returncode = 127
            self._streams = {1: self._stream(b''), 2: self._stream(b'')}
            if isinstance(error, FileNotFoundError):
                name = os.fsdecode(self._argv[0])
                raise FileNotFoundError(errno.ENOENT, 'No such file or directory', name) from None
            raise
        self.returncode = status
        self._streams = {1: self._stream(out), 2: self._stream(err)}
        _deliver(self._stdout, 1, out)
        if not merged:
            _deliver(self._stderr, 2, err)

    def _stream(self, data):
        if not self._text:
            return io.BytesIO(data)
        text = data.decode(self.encoding or 'utf-8', self.errors or 'strict')
        return io.StringIO(text.replace('\r\n', '\n').replace('\r', '\n'))

    def communicate(self, input=None, timeout=None):
        if input is not None and self.stdin is not None and not self.stdin.closed:
            self.stdin.write(input)
        self._finish()
        out = self._streams[1].read() if self._stdout == PIPE else None
        err = self._streams[2].read() if self._stderr == PIPE else None
        return out, err

    def wait(self, timeout=None):
        self._finish()
        return self.returncode

    def poll(self):
        self._finish()
        return self.returncode

    def send_signal(self, signal):
        pass

    def terminate(self):
        pass

    def kill(self):
        pass

    def __enter__(self):
        return self

    def __exit__(self, *error):
        self._finish()

    def __repr__(self):
        return '<Popen: returncode: %s args: %r>' % (self.returncode, self.args)


def run(*popenargs, input=None, capture_output=False, timeout=None, check=False, **kwargs):
    if input is not None:
        if kwargs.get('stdin') is not None:
            raise ValueError('stdin and input arguments may not both be used.')
        kwargs['stdin'] = PIPE
    if capture_output:
        if kwargs.get('stdout') is not None or kwargs.get('stderr') is not None:
            raise ValueError('stdout and stderr arguments may not be used with capture_output.')
        kwargs['stdout'] = PIPE
        kwargs['stderr'] = PIPE
    process = Popen(*popenargs, **kwargs)
    stdout, stderr = process.communicate(input, timeout=timeout)
    if check and process.returncode:
        raise CalledProcessError(process.returncode, process.args, output=stdout, stderr=stderr)
    return CompletedProcess(process.args, process.returncode, stdout, stderr)


def call(*popenargs, timeout=None, **kwargs):
    return Popen(*popenargs, **kwargs).wait(timeout=timeout)


def check_call(*popenargs, **kwargs):
    status = call(*popenargs, **kwargs)
    if status:
        cmd = kwargs.get('args')
        raise CalledProcessError(status, popenargs[0] if cmd is None else cmd)
    return 0


def check_output(*popenargs, timeout=None, **kwargs):
    if 'stdout' in kwargs:
        raise ValueError('stdout argument not allowed, it will be overridden.')
    if 'input' in kwargs and kwargs['input'] is None:
        kwargs['input'] = '' if kwargs.get('universal_newlines') or kwargs.get('text') or kwargs.get('encoding') or kwargs.get('errors') else b''
    return run(*popenargs, stdout=PIPE, timeout=timeout, check=True, **kwargs).stdout


def getstatusoutput(cmd, *, encoding=None, errors=None):
    try:
        data = check_output(cmd, shell=True, text=True, stderr=STDOUT, encoding=encoding, errors=errors)
        status = 0
    except CalledProcessError as error:
        data = error.output
        status = error.returncode
    if data[-1:] == '\n':
        data = data[:-1]
    return status, data


def getoutput(cmd, *, encoding=None, errors=None):
    return getstatusoutput(cmd, encoding=encoding, errors=errors)[1]
