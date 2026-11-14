#include <Python.h>
#include <stdarg.h>

typedef struct {
    char *data;
    Py_ssize_t length;
    Py_ssize_t capacity;
} Text;

static int text_reserve(Text *text, Py_ssize_t more) {
    if (text->length + more <= text->capacity)
        return 0;
    Py_ssize_t capacity = text->capacity ? text->capacity : 64;
    while (capacity < text->length + more)
        capacity *= 2;
    char *data = PyMem_Realloc(text->data, (size_t)capacity);
    if (!data) {
        PyErr_NoMemory();
        return -1;
    }
    text->data = data;
    text->capacity = capacity;
    return 0;
}

static int text_add(Text *text, const char *data, Py_ssize_t length) {
    if (text_reserve(text, length) < 0)
        return -1;
    for (Py_ssize_t index = 0; index < length; index++)
        text->data[text->length + index] = data[index];
    text->length += length;
    return 0;
}

static Py_ssize_t span(const char *data) {
    Py_ssize_t length = 0;
    while (data[length])
        length++;
    return length;
}

static int text_pad(Text *text, Py_ssize_t width, Py_ssize_t length, char fill) {
    while (width > length) {
        if (text_add(text, &fill, 1) < 0)
            return -1;
        width--;
    }
    return 0;
}

static int text_unsigned(Text *text, unsigned long long value, unsigned base, int upper, int negative, Py_ssize_t width, char fill) {
    char digits[sizeof(unsigned long long) * 8 + 1];
    const char *alphabet = upper ? "0123456789ABCDEF" : "0123456789abcdef";
    Py_ssize_t at = (Py_ssize_t)sizeof(digits);
    do {
        digits[--at] = alphabet[value % base];
        value /= base;
    } while (value);
    if (negative)
        digits[--at] = '-';
    Py_ssize_t length = (Py_ssize_t)sizeof(digits) - at;
    if (text_pad(text, width, length, fill) < 0)
        return -1;
    return text_add(text, digits + at, length);
}

static int text_object(Text *text, PyObject *object, Py_ssize_t precision) {
    if (!object)
        return -1;
    Py_ssize_t length;
    const char *data = PyUnicode_AsUTF8AndSize(object, &length);
    int result = -1;
    if (data) {
        if (precision >= 0 && precision < length)
            length = precision;
        result = text_add(text, data, length);
    }
    Py_DECREF(object);
    return result;
}

PyObject *PyUnicode_FromFormatV(const char *format, va_list arguments) {
    Text text = {0};
    va_list va;
    va_copy(va, arguments);
    const char *at = format;
    while (*at) {
        if (*at != '%') {
            const char *start = at;
            while (*at && *at != '%')
                at++;
            if (text_add(&text, start, at - start) < 0)
                goto fail;
            continue;
        }
        at++;
        if (*at == '%') {
            if (text_add(&text, "%", 1) < 0)
                goto fail;
            at++;
            continue;
        }
        char fill = ' ';
        while (*at == '-' || *at == '0') {
            if (*at == '0')
                fill = '0';
            at++;
        }
        Py_ssize_t width = 0;
        while (*at >= '0' && *at <= '9')
            width = width * 10 + (*at++ - '0');
        Py_ssize_t precision = -1;
        if (*at == '.') {
            at++;
            precision = 0;
            while (*at >= '0' && *at <= '9')
                precision = precision * 10 + (*at++ - '0');
        }
        int longs = 0;
        int sized = 0;
        if (*at == 'l') {
            longs = 1;
            at++;
            if (*at == 'l') {
                longs = 2;
                at++;
            }
        } else if (*at == 'z' || *at == 't' || *at == 'j') {
            sized = *at;
            at++;
        }
        char conversion = *at++;
        switch (conversion) {
        case 'd':
        case 'i': {
            long long value = longs == 2 ? va_arg(va, long long) : longs == 1 ? va_arg(va, long) : sized == 'j' ? (long long)va_arg(va, long long) : sized ? (long long)va_arg(va, Py_ssize_t) : va_arg(va, int);
            unsigned long long magnitude = value < 0 ? 0ULL - (unsigned long long)value : (unsigned long long)value;
            if (text_unsigned(&text, magnitude, 10, 0, value < 0, width, fill) < 0)
                goto fail;
            break;
        }
        case 'u':
        case 'x':
        case 'X':
        case 'o': {
            unsigned long long value = longs == 2 ? va_arg(va, unsigned long long) : longs == 1 ? va_arg(va, unsigned long) : sized == 'j' ? va_arg(va, unsigned long long) : sized ? (unsigned long long)va_arg(va, size_t) : va_arg(va, unsigned int);
            unsigned base = conversion == 'u' ? 10 : conversion == 'o' ? 8 : 16;
            if (text_unsigned(&text, value, base, conversion == 'X', 0, width, fill) < 0)
                goto fail;
            break;
        }
        case 'c': {
            int value = va_arg(va, int);
            PyObject *character = PyUnicode_FromOrdinal(value);
            if (text_object(&text, character, -1) < 0)
                goto fail;
            break;
        }
        case 'p': {
            void *value = va_arg(va, void *);
            if (text_add(&text, "0x", 2) < 0 || text_unsigned(&text, (unsigned long long)(size_t)value, 16, 0, 0, 0, ' ') < 0)
                goto fail;
            break;
        }
        case 's': {
            const char *value = va_arg(va, const char *);
            if (!value)
                value = "(null)";
            Py_ssize_t length = span(value);
            if (precision >= 0 && precision < length)
                length = precision;
            if (text_pad(&text, width, length, ' ') < 0 || text_add(&text, value, length) < 0)
                goto fail;
            break;
        }
        case 'U': {
            PyObject *value = va_arg(va, PyObject *);
            Py_INCREF(value);
            if (text_object(&text, value, precision) < 0)
                goto fail;
            break;
        }
        case 'V': {
            PyObject *value = va_arg(va, PyObject *);
            const char *fallback = va_arg(va, const char *);
            if (value) {
                Py_INCREF(value);
                if (text_object(&text, value, precision) < 0)
                    goto fail;
            } else if (text_add(&text, fallback, span(fallback)) < 0)
                goto fail;
            break;
        }
        case 'S':
            if (text_object(&text, PyObject_Str(va_arg(va, PyObject *)), precision) < 0)
                goto fail;
            break;
        case 'R':
            if (text_object(&text, PyObject_Repr(va_arg(va, PyObject *)), precision) < 0)
                goto fail;
            break;
        case 'A':
            if (text_object(&text, PyObject_ASCII(va_arg(va, PyObject *)), precision) < 0)
                goto fail;
            break;
        case 'T': {
            PyObject *value = va_arg(va, PyObject *);
            const char *name = Py_TYPE(value)->tp_name;
            if (text_add(&text, name, span(name)) < 0)
                goto fail;
            break;
        }
        default:
            va_end(va);
            PyMem_Free(text.data);
            PyErr_SetString(PyExc_SystemError, "PyUnicode_FromFormatV() got an unsupported format code");
            return NULL;
        }
    }
    va_end(va);
    PyObject *result = PyUnicode_FromStringAndSize(text.data ? text.data : "", text.length);
    PyMem_Free(text.data);
    return result;
fail:
    va_end(va);
    PyMem_Free(text.data);
    return NULL;
}

PyObject *PyUnicode_FromFormat(const char *format, ...) {
    va_list va;
    va_start(va, format);
    PyObject *result = PyUnicode_FromFormatV(format, va);
    va_end(va);
    return result;
}

PyObject *PyErr_FormatV(PyObject *exception, const char *format, va_list arguments) {
    PyObject *message = PyUnicode_FromFormatV(format, arguments);
    if (message) {
        PyErr_SetObject(exception, message);
        Py_DECREF(message);
    }
    return NULL;
}

PyObject *PyErr_Format(PyObject *exception, const char *format, ...) {
    va_list va;
    va_start(va, format);
    PyErr_FormatV(exception, format, va);
    va_end(va);
    return NULL;
}

static PyObject *build_value(const char **format, va_list *va, char end);

static PyObject *build_sequence(const char **format, va_list *va, char end, int list) {
    PyObject *items = PyList_New(0);
    if (!items)
        return NULL;
    while (**format && **format != end) {
        if (**format == ',' || **format == ' ' || **format == ':') {
            (*format)++;
            continue;
        }
        PyObject *item = build_value(format, va, end);
        if (!item || PyList_Append(items, item) < 0) {
            Py_XDECREF(item);
            Py_DECREF(items);
            return NULL;
        }
        Py_DECREF(item);
    }
    if (**format == end)
        (*format)++;
    if (list)
        return items;
    PyObject *tuple = PyList_AsTuple(items);
    Py_DECREF(items);
    return tuple;
}

static PyObject *build_dict(const char **format, va_list *va) {
    PyObject *dict = PyDict_New();
    if (!dict)
        return NULL;
    while (**format && **format != '}') {
        if (**format == ',' || **format == ' ' || **format == ':') {
            (*format)++;
            continue;
        }
        PyObject *key = build_value(format, va, '}');
        while (**format == ',' || **format == ' ' || **format == ':')
            (*format)++;
        PyObject *value = key ? build_value(format, va, '}') : NULL;
        if (!value || PyDict_SetItem(dict, key, value) < 0) {
            Py_XDECREF(key);
            Py_XDECREF(value);
            Py_DECREF(dict);
            return NULL;
        }
        Py_DECREF(key);
        Py_DECREF(value);
    }
    if (**format == '}')
        (*format)++;
    return dict;
}

static PyObject *build_text(const char **format, va_list *va, int bytes) {
    const char *value = va_arg(*va, const char *);
    Py_ssize_t length = -1;
    if (**format == '#') {
        (*format)++;
        length = va_arg(*va, Py_ssize_t);
    }
    if (!value)
        Py_RETURN_NONE;
    if (length < 0)
        length = span(value);
    return bytes ? PyBytes_FromStringAndSize(value, length) : PyUnicode_FromStringAndSize(value, length);
}

static PyObject *build_value(const char **format, va_list *va, char end) {
    char code = *(*format)++;
    switch (code) {
    case '(':
        return build_sequence(format, va, ')', 0);
    case '[':
        return build_sequence(format, va, ']', 1);
    case '{':
        return build_dict(format, va);
    case 'b':
    case 'B':
    case 'h':
    case 'i':
        return PyLong_FromLong(va_arg(*va, int));
    case 'H':
        return PyLong_FromLong((unsigned short)va_arg(*va, int));
    case 'I':
        return PyLong_FromUnsignedLong(va_arg(*va, unsigned int));
    case 'n':
        return PyLong_FromSsize_t(va_arg(*va, Py_ssize_t));
    case 'l':
        return PyLong_FromLong(va_arg(*va, long));
    case 'k':
        return PyLong_FromUnsignedLong(va_arg(*va, unsigned long));
    case 'L':
        return PyLong_FromLongLong(va_arg(*va, long long));
    case 'K':
        return PyLong_FromUnsignedLongLong(va_arg(*va, unsigned long long));
    case 'd':
    case 'f':
        return PyFloat_FromDouble(va_arg(*va, double));
    case 'c': {
        char value = (char)va_arg(*va, int);
        return PyBytes_FromStringAndSize(&value, 1);
    }
    case 'C':
        return PyUnicode_FromOrdinal(va_arg(*va, int));
    case 'p':
        return PyBool_FromLong(va_arg(*va, int));
    case 's':
    case 'z':
    case 'U':
        return build_text(format, va, 0);
    case 'y':
        return build_text(format, va, 1);
    case 'O':
    case 'S':
    case 'N': {
        if (**format == '&') {
            (*format)++;
            PyObject *(*converter)(void *) = va_arg(*va, PyObject *(*)(void *));
            return converter(va_arg(*va, void *));
        }
        PyObject *value = va_arg(*va, PyObject *);
        if (!value) {
            if (!PyErr_Occurred())
                PyErr_SetString(PyExc_SystemError, "NULL object passed to Py_BuildValue");
            return NULL;
        }
        if (code != 'N')
            Py_INCREF(value);
        return value;
    }
    default:
        (void)end;
        PyErr_SetString(PyExc_SystemError, "bad format char passed to Py_BuildValue");
        return NULL;
    }
}

static Py_ssize_t count_items(const char *format) {
    Py_ssize_t count = 0;
    int depth = 0;
    for (; *format; format++) {
        char code = *format;
        if (code == '(' || code == '[' || code == '{') {
            if (depth == 0)
                count++;
            depth++;
        } else if (code == ')' || code == ']' || code == '}')
            depth--;
        else if (depth == 0 && code != ',' && code != ' ' && code != ':' && code != '#' && code != '&' && code != '\t')
            count++;
    }
    return count;
}

PyObject *Py_VaBuildValue(const char *format, va_list arguments) {
    va_list va;
    va_copy(va, arguments);
    Py_ssize_t count = count_items(format);
    PyObject *result;
    if (count == 0) {
        result = Py_None;
        Py_INCREF(result);
    } else if (count == 1) {
        while (*format == ' ' || *format == ',' || *format == ':')
            format++;
        result = build_value(&format, &va, 0);
    } else
        result = build_sequence(&format, &va, 0, 0);
    va_end(va);
    return result;
}

PyObject *Py_BuildValue(const char *format, ...) {
    va_list va;
    va_start(va, format);
    PyObject *result = Py_VaBuildValue(format, va);
    va_end(va);
    return result;
}

PyObject *_Py_BuildValue_SizeT(const char *format, ...) {
    va_list va;
    va_start(va, format);
    PyObject *result = Py_VaBuildValue(format, va);
    va_end(va);
    return result;
}

static const char *function_name(const char *format) {
    while (*format && *format != ':' && *format != ';')
        format++;
    return *format == ':' ? format + 1 : "function";
}

static int type_error(const char *name, const char *expected, PyObject *got) {
    PyErr_Format(PyExc_TypeError, "%s() argument must be %s, not %T", name, expected, got);
    return -1;
}

static int convert(PyObject *arg, const char **format, va_list *va, const char *name) {
    char code = *(*format)++;
    switch (code) {
    case 'O': {
        if (**format == '!') {
            (*format)++;
            PyTypeObject *type = va_arg(*va, PyTypeObject *);
            PyObject **out = va_arg(*va, PyObject **);
            if (!PyObject_TypeCheck(arg, type))
                return type_error(name, type->tp_name, arg);
            *out = arg;
            return 0;
        }
        if (**format == '&') {
            (*format)++;
            int (*converter)(PyObject *, void *) = va_arg(*va, int (*)(PyObject *, void *));
            void *out = va_arg(*va, void *);
            return converter(arg, out) ? 0 : -1;
        }
        *va_arg(*va, PyObject **) = arg;
        return 0;
    }
    case 'S':
        if (!PyBytes_Check(arg))
            return type_error(name, "bytes", arg);
        *va_arg(*va, PyObject **) = arg;
        return 0;
    case 'U':
        if (!PyUnicode_Check(arg))
            return type_error(name, "str", arg);
        *va_arg(*va, PyObject **) = arg;
        return 0;
    case 'n': {
        Py_ssize_t value = PyLong_AsSsize_t(arg);
        if (value == -1 && PyErr_Occurred())
            return -1;
        *va_arg(*va, Py_ssize_t *) = value;
        return 0;
    }
    case 'b':
    case 'B':
    case 'h':
    case 'H':
    case 'i':
    case 'I':
    case 'l':
    case 'k': {
        long value = (code == 'k' || code == 'I' || code == 'B' || code == 'H') ? (long)PyLong_AsUnsignedLongMask(arg) : PyLong_AsLong(arg);
        if (value == -1 && PyErr_Occurred())
            return -1;
        if (code == 'b' || code == 'B')
            *va_arg(*va, unsigned char *) = (unsigned char)value;
        else if (code == 'h' || code == 'H')
            *va_arg(*va, short *) = (short)value;
        else if (code == 'i' || code == 'I')
            *va_arg(*va, int *) = (int)value;
        else
            *va_arg(*va, long *) = value;
        return 0;
    }
    case 'L':
    case 'K': {
        long long value = code == 'K' ? (long long)PyLong_AsUnsignedLongLongMask(arg) : PyLong_AsLongLong(arg);
        if (value == -1 && PyErr_Occurred())
            return -1;
        *va_arg(*va, long long *) = value;
        return 0;
    }
    case 'd':
    case 'f': {
        double value = PyFloat_AsDouble(arg);
        if (value == -1.0 && PyErr_Occurred())
            return -1;
        if (code == 'd')
            *va_arg(*va, double *) = value;
        else
            *va_arg(*va, float *) = (float)value;
        return 0;
    }
    case 'p': {
        int value = PyObject_IsTrue(arg);
        if (value < 0)
            return -1;
        *va_arg(*va, int *) = value;
        return 0;
    }
    case 'C': {
        if (!PyUnicode_Check(arg) || PyUnicode_GetLength(arg) != 1)
            return type_error(name, "a unicode character", arg);
        *va_arg(*va, int *) = (int)PyUnicode_ReadChar(arg, 0);
        return 0;
    }
    case 's':
    case 'z':
    case 'y': {
        if (**format == '*') {
            (*format)++;
            Py_buffer *view = va_arg(*va, Py_buffer *);
            if (code == 's' && PyUnicode_Check(arg)) {
                Py_ssize_t length;
                const char *data = PyUnicode_AsUTF8AndSize(arg, &length);
                if (!data)
                    return -1;
                return PyBuffer_FillInfo(view, arg, (void *)data, length, 1, 0);
            }
            return PyObject_GetBuffer(arg, view, PyBUF_SIMPLE);
        }
        const char **out = va_arg(*va, const char **);
        Py_ssize_t *size = NULL;
        if (**format == '#') {
            (*format)++;
            size = va_arg(*va, Py_ssize_t *);
        }
        if (code == 'z' && arg == Py_None) {
            *out = NULL;
            if (size)
                *size = 0;
            return 0;
        }
        Py_ssize_t length;
        const char *data;
        if (code != 'y' && PyUnicode_Check(arg))
            data = PyUnicode_AsUTF8AndSize(arg, &length);
        else if (PyBytes_Check(arg)) {
            data = PyBytes_AS_STRING(arg);
            length = PyBytes_GET_SIZE(arg);
        } else
            return type_error(name, code == 'y' ? "bytes" : "str", arg);
        if (!data)
            return -1;
        *out = data;
        if (size)
            *size = length;
        return 0;
    }
    default:
        PyErr_Format(PyExc_SystemError, "%s(): unsupported argument format '%c'", name, code);
        return -1;
    }
}

static void skip(const char **format, va_list *va) {
    char code = *(*format)++;
    if (code == 'O' && (**format == '!' || **format == '&')) {
        (*format)++;
        (void)va_arg(*va, void *);
    }
    (void)va_arg(*va, void *);
    if ((code == 's' || code == 'z' || code == 'y') && (**format == '#' || **format == '*')) {
        if (*(*format)++ == '#')
            (void)va_arg(*va, Py_ssize_t *);
    }
}

static int parse(PyObject *args, PyObject *kwargs, const char *format, char *const *names, va_list *va) {
    const char *name = function_name(format);
    if (!PyTuple_Check(args)) {
        PyErr_SetString(PyExc_SystemError, "argument list is not a tuple");
        return 0;
    }
    Py_ssize_t given = PyTuple_GET_SIZE(args);
    Py_ssize_t used = 0;
    Py_ssize_t index = 0;
    int optional = 0;
    Py_ssize_t keywords = kwargs ? PyDict_Size(kwargs) : 0;
    const char *at = format;
    while (*at && *at != ':' && *at != ';') {
        if (*at == '|') {
            optional = 1;
            at++;
            continue;
        }
        if (*at == '$') {
            at++;
            continue;
        }
        PyObject *arg = NULL;
        if (index < given)
            arg = PyTuple_GET_ITEM(args, index);
        else if (keywords && names && names[index] && names[index][0]) {
            arg = PyDict_GetItemString(kwargs, names[index]);
            if (arg)
                used++;
        }
        if (!arg) {
            if (!optional) {
                if (names && names[index] && names[index][0])
                    PyErr_Format(PyExc_TypeError, "%s() missing required argument '%s' (pos %zd)", name, names[index], index + 1);
                else
                    PyErr_Format(PyExc_TypeError, "%s() takes at least %zd positional arguments (%zd given)", name, index + 1, given);
                return 0;
            }
            skip(&at, va);
        } else if (convert(arg, &at, va, name) < 0)
            return 0;
        index++;
    }
    if (given > index) {
        PyErr_Format(PyExc_TypeError, "%s() takes at most %zd arguments (%zd given)", name, index, given);
        return 0;
    }
    if (used < keywords) {
        PyObject *key;
        PyObject *value;
        Py_ssize_t position = 0;
        while (PyDict_Next(kwargs, &position, &key, &value)) {
            int known = 0;
            for (Py_ssize_t slot = 0; names && slot < index && names[slot]; slot++)
                if (PyUnicode_CompareWithASCIIString(key, names[slot]) == 0)
                    known = slot >= given;
            if (!known) {
                PyErr_Format(PyExc_TypeError, "%s() got an unexpected keyword argument '%U'", name, key);
                return 0;
            }
        }
    }
    return 1;
}

int PyArg_VaParse(PyObject *args, const char *format, va_list arguments) {
    va_list va;
    va_copy(va, arguments);
    int result = parse(args, NULL, format, NULL, &va);
    va_end(va);
    return result;
}

int PyArg_ParseTuple(PyObject *args, const char *format, ...) {
    va_list va;
    va_start(va, format);
    int result = parse(args, NULL, format, NULL, &va);
    va_end(va);
    return result;
}

int _PyArg_ParseTuple_SizeT(PyObject *args, const char *format, ...) {
    va_list va;
    va_start(va, format);
    int result = parse(args, NULL, format, NULL, &va);
    va_end(va);
    return result;
}

int PyArg_VaParseTupleAndKeywords(PyObject *args, PyObject *kwargs, const char *format, char *const *names, va_list arguments) {
    va_list va;
    va_copy(va, arguments);
    int result = parse(args, kwargs, format, names, &va);
    va_end(va);
    return result;
}

int PyArg_ParseTupleAndKeywords(PyObject *args, PyObject *kwargs, const char *format, char *const *names, ...) {
    va_list va;
    va_start(va, names);
    int result = parse(args, kwargs, format, names, &va);
    va_end(va);
    return result;
}

int _PyArg_ParseTupleAndKeywords_SizeT(PyObject *args, PyObject *kwargs, const char *format, char *const *names, ...) {
    va_list va;
    va_start(va, names);
    int result = parse(args, kwargs, format, names, &va);
    va_end(va);
    return result;
}

int PyArg_UnpackTuple(PyObject *args, const char *name, Py_ssize_t min, Py_ssize_t max, ...) {
    Py_ssize_t given = PyTuple_GET_SIZE(args);
    if (given < min || given > max) {
        PyErr_Format(PyExc_TypeError, "%s expected %s%zd argument%s, got %zd", name, given < min ? (min == max ? "" : "at least ") : (min == max ? "" : "at most "), given < min ? min : max, (given < min ? min : max) == 1 ? "" : "s", given);
        return 0;
    }
    va_list va;
    va_start(va, max);
    for (Py_ssize_t index = 0; index < given; index++)
        *va_arg(va, PyObject **) = PyTuple_GET_ITEM(args, index);
    va_end(va);
    return 1;
}

PyObject *PyTuple_Pack(Py_ssize_t count, ...) {
    PyObject *tuple = PyTuple_New(count);
    if (!tuple)
        return NULL;
    va_list va;
    va_start(va, count);
    for (Py_ssize_t index = 0; index < count; index++) {
        PyObject *item = va_arg(va, PyObject *);
        Py_INCREF(item);
        PyTuple_SET_ITEM(tuple, index, item);
    }
    va_end(va);
    return tuple;
}

static PyObject *call_format(PyObject *callable, const char *format, va_list *va) {
    PyObject *args;
    if (!format || !*format)
        args = PyTuple_New(0);
    else {
        args = Py_VaBuildValue(format, *va);
        if (args && !PyTuple_Check(args)) {
            PyObject *single = PyTuple_Pack(1, args);
            Py_DECREF(args);
            args = single;
        }
    }
    if (!args)
        return NULL;
    PyObject *result = PyObject_Call(callable, args, NULL);
    Py_DECREF(args);
    return result;
}

PyObject *PyObject_CallFunction(PyObject *callable, const char *format, ...) {
    va_list va;
    va_start(va, format);
    PyObject *result = call_format(callable, format, &va);
    va_end(va);
    return result;
}

PyObject *PyObject_CallMethod(PyObject *object, const char *name, const char *format, ...) {
    PyObject *method = PyObject_GetAttrString(object, name);
    if (!method)
        return NULL;
    va_list va;
    va_start(va, format);
    PyObject *result = call_format(method, format, &va);
    va_end(va);
    Py_DECREF(method);
    return result;
}

PyObject *_PyObject_CallMethod_SizeT(PyObject *object, const char *name, const char *format, ...) {
    PyObject *method = PyObject_GetAttrString(object, name);
    if (!method)
        return NULL;
    va_list va;
    va_start(va, format);
    PyObject *result = call_format(method, format, &va);
    va_end(va);
    Py_DECREF(method);
    return result;
}

static PyObject *collect(va_list *va) {
    PyObject *items = PyList_New(0);
    if (!items)
        return NULL;
    for (PyObject *item = va_arg(*va, PyObject *); item; item = va_arg(*va, PyObject *))
        if (PyList_Append(items, item) < 0) {
            Py_DECREF(items);
            return NULL;
        }
    PyObject *tuple = PyList_AsTuple(items);
    Py_DECREF(items);
    return tuple;
}

PyObject *PyObject_CallFunctionObjArgs(PyObject *callable, ...) {
    va_list va;
    va_start(va, callable);
    PyObject *args = collect(&va);
    va_end(va);
    if (!args)
        return NULL;
    PyObject *result = PyObject_Call(callable, args, NULL);
    Py_DECREF(args);
    return result;
}

PyObject *PyObject_CallMethodObjArgs(PyObject *object, PyObject *name, ...) {
    PyObject *method = PyObject_GetAttr(object, name);
    if (!method)
        return NULL;
    va_list va;
    va_start(va, name);
    PyObject *args = collect(&va);
    va_end(va);
    PyObject *result = args ? PyObject_Call(method, args, NULL) : NULL;
    Py_XDECREF(args);
    Py_DECREF(method);
    return result;
}
