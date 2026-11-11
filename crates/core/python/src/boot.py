import sys
import _io
import _host
import posix
import builtins
import time
import _frozen_importlib

sys.builtin_module_names = tuple(sys.builtin_module_names) + ('posix',)


class FileIO(_io._RawIOBase):
    def __init__(self, file, mode='r', closefd=True, opener=None):
        self._fd = -1
        if isinstance(file, float):
            raise TypeError('integer argument expected, got float')
        if not isinstance(mode, str):
            raise TypeError('invalid mode: %r' % (mode,))
        if not set(mode) <= set('xrwab+') or sum(c in 'rwax' for c in mode) != 1 or mode.count('+') > 1:
            raise ValueError('Must have exactly one of create/read/write/append mode and at most one plus')
        plus = '+' in mode
        if 'x' in mode:
            flags = posix.O_EXCL | posix.O_CREAT
        elif 'w' in mode:
            flags = posix.O_CREAT | posix.O_TRUNC
        elif 'a' in mode:
            flags = posix.O_APPEND | posix.O_CREAT
        else:
            flags = 0
        self._readable = 'r' in mode or plus
        self._writable = 'r' not in mode or plus
        flags |= posix.O_RDWR if self._readable and self._writable else posix.O_RDONLY if self._readable else posix.O_WRONLY
        self._mode = ('x' if 'x' in mode else 'r' if 'r' in mode else 'w' if 'w' in mode else 'a') + 'b' + ('+' if plus else '')
        self._closefd = closefd
        if isinstance(file, int):
            if file < 0:
                raise ValueError('negative file descriptor')
            self._fd = file
        else:
            if not closefd:
                raise ValueError('Cannot use closefd=False with file name')
            if opener is None:
                self._fd = posix.open(file, flags, 0o666)
            else:
                self._fd = opener(file, flags)
            if posix.fstat(self._fd).st_mode & 0o170000 == 0o040000:
                posix.close(self._fd)
                self._fd = -1
                raise IsADirectoryError(posix._errno.EISDIR, 'Is a directory', file)
        self.name = file

    mode = property(lambda self: self._mode)
    closefd = property(lambda self: self._closefd)

    def _open(self):
        if self._fd < 0:
            raise ValueError('I/O operation on closed file')
        return self._fd

    def readable(self):
        self._open()
        return self._readable

    def writable(self):
        self._open()
        return self._writable

    def seekable(self):
        return self._open() > 2

    def fileno(self):
        return self._open()

    def isatty(self):
        self._open()
        return False

    def read(self, size=-1):
        if not self._readable:
            raise _io.UnsupportedOperation('File not open for reading')
        if size is None or size < 0:
            return posix._readall(self._open())
        return posix.read(self._open(), size)

    def readall(self):
        return self.read()

    def readinto(self, buffer):
        data = self.read(len(buffer))
        size = len(data)
        buffer[:size] = data
        return size

    def write(self, data):
        if not self._writable:
            raise _io.UnsupportedOperation('File not open for writing')
        return posix.write(self._open(), data)

    def seek(self, pos, whence=0):
        return posix.lseek(self._open(), pos, whence)

    def tell(self):
        return posix.lseek(self._open(), 0, 1)

    def truncate(self, size=None):
        if size is None:
            size = self.tell()
        posix.ftruncate(self._open(), size)
        return size

    def close(self):
        if self._fd < 0:
            return
        fd, self._fd = self._fd, -1
        try:
            _io._RawIOBase.close(self)
        finally:
            if self._closefd:
                posix.close(fd)

    def __repr__(self):
        return '<_io.FileIO name=%r mode=%r closefd=%r>' % (self.name, self._mode, self._closefd)


def open(file, mode='r', buffering=-1, encoding=None, errors=None, newline=None, closefd=True, opener=None):
    if not isinstance(file, int):
        file = posix.fspath(file)
    if not isinstance(mode, str):
        raise TypeError('open() argument \'mode\' must be str, not ' + type(mode).__name__)
    modes = set(mode)
    if modes - set('axrwb+t') or len(mode) > len(modes):
        raise ValueError('invalid mode: %r' % (mode,))
    creating, reading, writing, appending = 'x' in modes, 'r' in modes, 'w' in modes, 'a' in modes
    updating, binary = '+' in modes, 'b' in modes
    if 't' in modes and binary:
        raise ValueError("can't have text and binary mode at once")
    if creating + reading + writing + appending != 1:
        raise ValueError('Must have exactly one of create/read/write/append mode and at most one plus')
    if binary and encoding is not None:
        raise ValueError("binary mode doesn't take an encoding argument")
    if binary and errors is not None:
        raise ValueError("binary mode doesn't take an errors argument")
    if binary and newline is not None:
        raise ValueError("binary mode doesn't take a newline argument")
    raw = FileIO(file, ('x' if creating else 'r' if reading else 'w' if writing else 'a') + ('+' if updating else ''), closefd, opener)
    result = raw
    try:
        if appending:
            raw.seek(0, 2)
        if buffering == 0:
            if binary:
                return raw
            raise ValueError("can't have unbuffered text I/O")
        size = _io.DEFAULT_BUFFER_SIZE if buffering < 2 else buffering
        if updating:
            result = _io.BufferedRandom(raw, size)
        elif reading:
            result = _io.BufferedReader(raw, size)
        else:
            result = _io.BufferedWriter(raw, size)
        if binary:
            return result
        result = _io.TextIOWrapper(result, encoding or 'utf-8', errors, newline, buffering == 1)
        result.mode = mode
        return result
    except BaseException:
        result.close()
        raise


def open_code(path):
    return open(path, 'rb')


_io.FileIO = FileIO
_io.open = open
_io.open_code = open_code
builtins.open = open
if 'io' in sys.modules:
    sys.modules['io'].FileIO = FileIO
    sys.modules['io'].open = open
    sys.modules['io'].open_code = open_code


class _Loader:
    def __init__(self, name, path, package):
        self.name = name
        self.path = path
        self.package = package

    def create_module(self, spec):
        return None

    def get_filename(self, name=None):
        return self.path

    def is_package(self, name=None):
        return self.package

    def get_source(self, name=None):
        with open(self.path, 'rb') as file:
            return file.read().decode('utf-8')

    def get_code(self, name=None):
        with open(self.path, 'rb') as file:
            return compile(file.read(), self.path, 'exec')

    def exec_module(self, module):
        module.__file__ = self.path
        exec(self.get_code(), module.__dict__)


_loaded = []


class _Finder:
    @staticmethod
    def find_spec(name, path=None, target=None):
        tail = name.rpartition('.')[2]
        for base in sys.path if path is None else path:
            if not isinstance(base, str):
                continue
            base = (base or '.') + '/' + tail
            for file, package in ((base + '/__init__.py', True), (base + '.py', False)):
                try:
                    if posix.stat(file).st_mode & 0o170000 != 0o100000:
                        continue
                except OSError:
                    continue
                spec = _frozen_importlib.ModuleSpec(name, _Loader(name, file, package), origin=file, is_package=package)
                if package:
                    spec.submodule_search_locations = [base]
                _loaded.append(name)
                return spec
        return None

    @staticmethod
    def invalidate_caches():
        pass


sys.meta_path.append(_Finder)


def _stdio(fd, name):
    raw = FileIO(fd, 'r' if fd == 0 else 'w', False)
    raw.name = name
    if fd == 0:
        text = _io.TextIOWrapper(_io.BufferedReader(raw), 'utf-8', 'surrogateescape', None, False)
    else:
        text = _io.TextIOWrapper(_io.BufferedWriter(raw), 'utf-8', 'backslashreplace' if fd == 2 else 'surrogateescape', '\n', fd == 2)
    text.mode = 'r' if fd == 0 else 'w'
    return text


_gmtime, _localtime, _strftime, _ctime = time.gmtime, time.localtime, time.strftime, time.ctime


def _gmtime_now(seconds=None):
    return _gmtime(_host.now() if seconds is None else seconds)


def _localtime_now(seconds=None):
    return _localtime(_host.now() if seconds is None else seconds)


def _strftime_now(format, moment=None):
    return _strftime(format, _localtime_now() if moment is None else moment)


def _ctime_now(seconds=None):
    return _ctime(_host.now() if seconds is None else seconds)


time.time = _host.now
time.time_ns = lambda: int(_host.now() * 1000000000)
time.monotonic = time.perf_counter = time.process_time = _host.monotonic
time.monotonic_ns = time.perf_counter_ns = time.process_time_ns = lambda: int(_host.monotonic() * 1000000000)
time.sleep = _host.sleep
time.gmtime = _gmtime_now
time.localtime = _localtime_now
time.strftime = _strftime_now
time.ctime = _ctime_now


def _start(argv, first):
    posix._reset()
    while _loaded:
        sys.modules.pop(_loaded.pop(), None)
    if 'os' in sys.modules:
        environ = sys.modules['os'].environ
        environ._data.clear()
        environ._data.update(posix.environ)
    sys.argv = argv
    sys.executable = sys._base_executable = ''
    sys.path[:] = [posix._abs(first).decode('utf-8', 'surrogateescape') if first else '']
    sys.stdin = sys.__stdin__ = _stdio(0, '<stdin>')
    sys.stdout = sys.__stdout__ = _stdio(1, '<stdout>')
    sys.stderr = sys.__stderr__ = _stdio(2, '<stderr>')


def _missing(names):
    import _imp
    missing = []
    for name in names:
        top = name.partition('.')[0]
        if name in sys.modules or _Finder.find_spec(top) is not None:
            continue
        try:
            __import__(name)
        except ImportError as error:
            missing += (name, 'standard' if _imp.is_builtin(top) or _imp.is_frozen(top) else 'package', str(error))
    return missing


def _absent(module):
    import _imp
    top = module.partition('.')[0]
    try:
        found = _frozen_importlib._find_spec(top, None) is not None
    except ImportError:
        found = False
    if found:
        return []
    return [module, 'standard' if _imp.is_builtin(top) or _imp.is_frozen(top) else 'package', 'No module named %r' % (top,)]


def _finish():
    try:
        import atexit
        atexit._run_exitfuncs()
    except BaseException:
        pass
    main = sys.modules.get('__main__')
    if main is not None:
        main.__dict__.clear()
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.flush()
        except BaseException:
            pass
