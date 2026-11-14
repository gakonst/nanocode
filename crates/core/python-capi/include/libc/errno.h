#ifndef _ERRNO_H
#define _ERRNO_H
int *__errno_location(void);
#define errno (*__errno_location())
#define EINTR 27
#define ENOMEM 48
#define ERANGE 68
#define EDOM 18
#define EAGAIN 6
#define EINVAL 28
#endif
