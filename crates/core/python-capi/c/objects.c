#include "Python.h"
#include "datetime.h"
#include <string.h>

static const char *start_mode(int start) {
    switch (start) {
    case Py_eval_input:
        return "eval";
    case Py_single_input:
        return "single";
    default:
        return "exec";
    }
}

PyObject *Py_CompileStringExFlags(const char *source, const char *filename, int start, PyCompilerFlags *flags, int optimize) {
    (void)flags;
    PyObject *compile = PyDict_GetItemString(PyEval_GetBuiltins(), "compile");
    if (compile == NULL) {
        PyErr_SetString(PyExc_SystemError, "builtins.compile is missing");
        return NULL;
    }
    return PyObject_CallFunction(compile, "sssiii", source, filename, start_mode(start), 0, 1, optimize);
}

PyObject *PyEval_EvalCode(PyObject *code, PyObject *globals, PyObject *locals) {
    PyObject *eval = PyDict_GetItemString(PyEval_GetBuiltins(), "eval");
    if (eval == NULL) {
        PyErr_SetString(PyExc_SystemError, "builtins.eval is missing");
        return NULL;
    }
    return PyObject_CallFunctionObjArgs(eval, code, globals, locals ? locals : globals, NULL);
}


static PyDateTime_CAPI datetime_api;
static PyObject *datetime_timezone;

PyTypeObject *_Py_datetime_type(int which) {
    if (datetime_api.DateType == NULL) {
        return NULL;
    }
    switch (which) {
    case 0: return datetime_api.DateTimeType;
    case 1: return datetime_api.DateType;
    case 2: return datetime_api.TimeType;
    default: return datetime_api.DeltaType;
    }
}

static PyObject *with_fold(PyObject *type, PyObject *args, int fold) {
    if (args == NULL) {
        return NULL;
    }
    PyObject *keywords = Py_BuildValue("{s:i}", "fold", fold);
    PyObject *result = keywords == NULL ? NULL : PyObject_Call(type, args, keywords);
    Py_DECREF(args);
    Py_XDECREF(keywords);
    return result;
}

static PyObject *date_from_date(int year, int month, int day, PyTypeObject *type) {
    return PyObject_CallFunction((PyObject *)type, "iii", year, month, day);
}

static PyObject *datetime_from_fold(int year, int month, int day, int hour, int minute, int second, int microsecond, PyObject *tz, int fold, PyTypeObject *type) {
    return with_fold((PyObject *)type, Py_BuildValue("iiiiiiiO", year, month, day, hour, minute, second, microsecond, tz), fold);
}

static PyObject *datetime_from(int year, int month, int day, int hour, int minute, int second, int microsecond, PyObject *tz, PyTypeObject *type) {
    return datetime_from_fold(year, month, day, hour, minute, second, microsecond, tz, 0, type);
}

static PyObject *time_from_fold(int hour, int minute, int second, int microsecond, PyObject *tz, int fold, PyTypeObject *type) {
    return with_fold((PyObject *)type, Py_BuildValue("iiiiO", hour, minute, second, microsecond, tz), fold);
}

static PyObject *time_from(int hour, int minute, int second, int microsecond, PyObject *tz, PyTypeObject *type) {
    return time_from_fold(hour, minute, second, microsecond, tz, 0, type);
}

static PyObject *delta_from(int days, int seconds, int microseconds, int normalize, PyTypeObject *type) {
    (void)normalize;
    return PyObject_CallFunction((PyObject *)type, "iii", days, seconds, microseconds);
}

static PyObject *timezone_from(PyObject *offset, PyObject *name) {
    return name == NULL ? PyObject_CallOneArg(datetime_timezone, offset) : PyObject_CallFunctionObjArgs(datetime_timezone, offset, name, NULL);
}

static PyObject *from_timestamp(PyObject *type, PyObject *args, PyObject *keywords) {
    PyObject *method = PyObject_GetAttrString(type, "fromtimestamp");
    PyObject *result = method == NULL ? NULL : PyObject_Call(method, args, keywords);
    Py_XDECREF(method);
    return result;
}

static PyObject *date_from_timestamp(PyObject *type, PyObject *args) {
    return from_timestamp(type, args, NULL);
}

static int datetime_type(PyObject *module, const char *name, PyTypeObject **slot) {
    PyObject *value = PyObject_GetAttrString(module, name);
    *slot = (PyTypeObject *)value;
    return value == NULL ? -1 : 0;
}

static void *datetime_capsule(void) {
    if (datetime_api.DateType != NULL) {
        return &datetime_api;
    }
    PyObject *module = PyImport_ImportModule("datetime");
    if (module == NULL) {
        return NULL;
    }
    if (datetime_type(module, "date", &datetime_api.DateType) < 0
        || datetime_type(module, "datetime", &datetime_api.DateTimeType) < 0
        || datetime_type(module, "time", &datetime_api.TimeType) < 0
        || datetime_type(module, "timedelta", &datetime_api.DeltaType) < 0
        || datetime_type(module, "tzinfo", &datetime_api.TZInfoType) < 0) {
        datetime_api.DateType = NULL;
        Py_DECREF(module);
        return NULL;
    }
    datetime_timezone = PyObject_GetAttrString(module, "timezone");
    datetime_api.TimeZone_UTC = datetime_timezone == NULL ? NULL : PyObject_GetAttrString(datetime_timezone, "utc");
    if (datetime_api.TimeZone_UTC == NULL) {
        datetime_api.DateType = NULL;
        Py_DECREF(module);
        return NULL;
    }
    datetime_api.Date_FromDate = date_from_date;
    datetime_api.DateTime_FromDateAndTime = datetime_from;
    datetime_api.Time_FromTime = time_from;
    datetime_api.Delta_FromDelta = delta_from;
    datetime_api.TimeZone_FromTimeZone = timezone_from;
    datetime_api.DateTime_FromTimestamp = from_timestamp;
    datetime_api.Date_FromTimestamp = date_from_timestamp;
    datetime_api.DateTime_FromDateAndTimeAndFold = datetime_from_fold;
    datetime_api.Time_FromTimeAndFold = time_from_fold;
    PyObject *capsule = PyCapsule_New(&datetime_api, PyDateTime_CAPSULE_NAME, NULL);
    if (capsule != NULL) {
        PyObject_SetAttrString(module, "datetime_CAPI", capsule);
        Py_DECREF(capsule);
    }
    Py_DECREF(module);
    PyErr_Clear();
    return &datetime_api;
}

void *PyCapsule_Import(const char *name, int no_block) {
    (void)no_block;
    size_t length = strlen(name);
    char *path = PyMem_Malloc(length + 1);
    if (path == NULL) {
        PyErr_NoMemory();
        return NULL;
    }
    memcpy(path, name, length + 1);
    char *dot = strchr(path, '.');
    if (dot != NULL) {
        *dot = 0;
    }
    PyObject *object = PyImport_ImportModule(path);
    while (object != NULL && dot != NULL) {
        char *part = dot + 1;
        *dot = '.';
        dot = strchr(part, '.');
        if (dot != NULL) {
            *dot = 0;
        }
        PyObject *next = PyObject_GetAttrString(object, part);
        if (next == NULL) {
            PyErr_Clear();
            next = PyImport_ImportModule(path);
        }
        Py_DECREF(object);
        object = next;
    }
    PyMem_Free(path);
    if (object == NULL) {
        if (strcmp(name, PyDateTime_CAPSULE_NAME) == 0) {
            PyErr_Clear();
            return datetime_capsule();
        }
        return NULL;
    }
    void *pointer = PyCapsule_GetPointer(object, name);
    Py_DECREF(object);
    return pointer;
}

PyObject *PyVectorcall_Call(PyObject *callable, PyObject *tuple, PyObject *keywords) {
    Py_ssize_t offset = Py_TYPE(callable)->tp_vectorcall_offset;
    vectorcallfunc function = offset > 0 ? *(vectorcallfunc *)((char *)callable + offset) : NULL;
    if (function == NULL) {
        PyErr_Format(PyExc_TypeError, "'%.200s' object does not support vectorcall", Py_TYPE(callable)->tp_name);
        return NULL;
    }
    Py_ssize_t positional = PyTuple_Size(tuple);
    Py_ssize_t named = keywords == NULL ? 0 : PyDict_Size(keywords);
    if (positional < 0 || named < 0) {
        return NULL;
    }
    PyObject **items = PyMem_Malloc(sizeof(PyObject *) * (size_t)(positional + named + 1));
    if (items == NULL) {
        return PyErr_NoMemory();
    }
    for (Py_ssize_t at = 0; at < positional; at++) {
        items[at] = PyTuple_GetItem(tuple, at);
    }
    PyObject *names = NULL;
    if (named > 0) {
        names = PyTuple_New(named);
        Py_ssize_t position = 0, at = 0;
        PyObject *key, *value;
        while (names != NULL && PyDict_Next(keywords, &position, &key, &value)) {
            Py_INCREF(key);
            PyTuple_SetItem(names, at, key);
            items[positional + at] = value;
            at++;
        }
        if (names == NULL) {
            PyMem_Free(items);
            return NULL;
        }
    }
    PyObject *result = function(callable, items, (size_t)positional, names);
    Py_XDECREF(names);
    PyMem_Free(items);
    return result;
}
