#ifndef NANOCODE_ARPA_INET_H
#define NANOCODE_ARPA_INET_H
#include <stdint.h>
static inline uint32_t htonl(uint32_t value) { return __builtin_bswap32(value); }
static inline uint16_t htons(uint16_t value) { return __builtin_bswap16(value); }
static inline uint32_t ntohl(uint32_t value) { return __builtin_bswap32(value); }
static inline uint16_t ntohs(uint16_t value) { return __builtin_bswap16(value); }
#endif
