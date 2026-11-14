#undef assert
#ifdef NDEBUG
#define assert(e) ((void)0)
#else
#define assert(e) ((e) ? (void)0 : __builtin_trap())
#endif
#ifndef __cplusplus
#define static_assert _Static_assert
#endif
