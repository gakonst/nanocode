#include "Python.h"
#include <stddef.h>
#include <string.h>

typedef struct {
    short subslot_offset;
    short slot_offset;
} SlotOffset;

static const SlotOffset slot_offsets[] = {
    {0, 0},
#include "typeslots.inc"
};

const unsigned long Py_Version = PY_VERSION_HEX;

static int special_member(const PyMemberDef *member, PyTypeObject *type) {
    if (strcmp(member->name, "__weaklistoffset__") == 0) {
        type->tp_weaklistoffset = member->offset;
        return 1;
    }
    if (strcmp(member->name, "__dictoffset__") == 0) {
        type->tp_dictoffset = member->offset;
        return 1;
    }
    if (strcmp(member->name, "__vectorcalloffset__") == 0) {
        type->tp_vectorcall_offset = member->offset;
        return 1;
    }
    return 0;
}

PyObject *PyType_FromMetaclass(PyTypeObject *metaclass, PyObject *module, PyType_Spec *spec, PyObject *bases) {
    (void)metaclass;
    PyHeapTypeObject *heap = PyObject_Calloc(1, sizeof(PyHeapTypeObject));
    if (heap == NULL) {
        return PyErr_NoMemory();
    }
    PyTypeObject *type = &heap->ht_type;
    Py_SET_TYPE(type, &PyType_Type);
    Py_SET_REFCNT(type, _Py_IMMORTAL_REFCNT);
    type->tp_name = spec->name;
    type->tp_basicsize = spec->basicsize;
    type->tp_itemsize = spec->itemsize;
    type->tp_flags = spec->flags | Py_TPFLAGS_HEAPTYPE;
    type->tp_as_async = &heap->as_async;
    type->tp_as_number = &heap->as_number;
    type->tp_as_sequence = &heap->as_sequence;
    type->tp_as_mapping = &heap->as_mapping;
    type->tp_as_buffer = &heap->as_buffer;
    PyTypeObject *base = NULL;
    if (bases != NULL) {
        if (PyType_Check(bases)) {
            base = (PyTypeObject *)bases;
        } else if (PyTuple_Check(bases) && PyTuple_GET_SIZE(bases) > 0) {
            base = (PyTypeObject *)PyTuple_GET_ITEM(bases, 0);
        }
    }
    for (PyType_Slot *slot = spec->slots; slot->slot; slot++) {
        if (slot->slot < 0 || (size_t)slot->slot >= Py_ARRAY_LENGTH(slot_offsets)) {
            PyErr_SetString(PyExc_RuntimeError, "invalid slot offset");
            return NULL;
        }
        switch (slot->slot) {
        case Py_tp_base:
            if (base == NULL) {
                base = slot->pfunc;
            }
            continue;
        case Py_tp_bases:
            if (base == NULL && PyTuple_Check((PyObject *)slot->pfunc) && PyTuple_GET_SIZE((PyObject *)slot->pfunc) > 0) {
                base = (PyTypeObject *)PyTuple_GET_ITEM((PyObject *)slot->pfunc, 0);
            }
            continue;
        case Py_tp_members:
            for (const PyMemberDef *member = slot->pfunc; member->name != NULL; member++) {
                special_member(member, type);
            }
            break;
        }
        SlotOffset offsets = slot_offsets[slot->slot];
        char *parent = (char *)type;
        if (offsets.subslot_offset != -1) {
            parent = *(char **)((char *)type + offsets.slot_offset);
            *(void **)(parent + offsets.subslot_offset) = slot->pfunc;
        } else {
            *(void **)(parent + offsets.slot_offset) = slot->pfunc;
        }
    }
    type->tp_base = base;
    heap->ht_module = module;
    Py_XINCREF(module);
    if (PyType_Ready(type) < 0) {
        return NULL;
    }
    return (PyObject *)type;
}

PyObject *PyType_FromModuleAndSpec(PyObject *module, PyType_Spec *spec, PyObject *bases) {
    return PyType_FromMetaclass(NULL, module, spec, bases);
}

PyObject *PyType_FromSpecWithBases(PyType_Spec *spec, PyObject *bases) {
    return PyType_FromMetaclass(NULL, NULL, spec, bases);
}

PyObject *PyType_FromSpec(PyType_Spec *spec) {
    return PyType_FromMetaclass(NULL, NULL, spec, NULL);
}

void *PyType_GetSlot(PyTypeObject *type, int slot) {
    if (slot <= 0 || (size_t)slot >= Py_ARRAY_LENGTH(slot_offsets)) {
        PyErr_BadInternalCall();
        return NULL;
    }
    void *parent = *(void **)((char *)type + slot_offsets[slot].slot_offset);
    if (parent == NULL || slot_offsets[slot].subslot_offset == -1) {
        return parent;
    }
    return *(void **)((char *)parent + slot_offsets[slot].subslot_offset);
}

PyObject *PyType_GetModule(PyTypeObject *type) {
    if (!(type->tp_flags & Py_TPFLAGS_HEAPTYPE) || ((PyHeapTypeObject *)type)->ht_module == NULL) {
        PyErr_Format(PyExc_TypeError, "PyType_GetModule: Type '%s' has no associated module", type->tp_name);
        return NULL;
    }
    return ((PyHeapTypeObject *)type)->ht_module;
}

void *PyType_GetModuleState(PyTypeObject *type) {
    PyObject *module = PyType_GetModule(type);
    return module == NULL ? NULL : PyModule_GetState(module);
}

PyObject *PyType_GetModuleByDef(PyTypeObject *type, PyModuleDef *def) {
    for (PyTypeObject *at = type; at != NULL; at = at->tp_base) {
        if ((at->tp_flags & Py_TPFLAGS_HEAPTYPE) && ((PyHeapTypeObject *)at)->ht_module != NULL
            && PyModule_GetDef(((PyHeapTypeObject *)at)->ht_module) == def) {
            return ((PyHeapTypeObject *)at)->ht_module;
        }
    }
    PyErr_Format(PyExc_TypeError, "PyType_GetModuleByDef: No superclass of '%s' has the given module", type->tp_name);
    return NULL;
}

int PyOS_vsnprintf(char *buffer, size_t size, const char *format, va_list arguments) {
    PyObject *text = PyUnicode_FromFormatV(format, arguments);
    if (text == NULL || size == 0) {
        Py_XDECREF(text);
        return -1;
    }
    Py_ssize_t length = 0;
    const char *data = PyUnicode_AsUTF8AndSize(text, &length);
    if (data == NULL) {
        Py_DECREF(text);
        return -1;
    }
    size_t copied = (size_t)length < size - 1 ? (size_t)length : size - 1;
    memcpy(buffer, data, copied);
    buffer[copied] = 0;
    Py_DECREF(text);
    return (int)length;
}

int PyOS_snprintf(char *buffer, size_t size, const char *format, ...) {
    va_list arguments;
    va_start(arguments, format);
    int written = PyOS_vsnprintf(buffer, size, format, arguments);
    va_end(arguments);
    return written;
}

int PyErr_WarnFormat(PyObject *category, Py_ssize_t level, const char *format, ...) {
    va_list arguments;
    va_start(arguments, format);
    PyObject *message = PyUnicode_FromFormatV(format, arguments);
    va_end(arguments);
    if (message == NULL) {
        return -1;
    }
    const char *data = PyUnicode_AsUTF8(message);
    int result = data == NULL ? -1 : PyErr_WarnEx(category, data, level);
    Py_DECREF(message);
    return result;
}
