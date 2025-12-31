#if defined(__ANDROID__)
#include <errno.h>
#include <stdio.h>
#include <sys/types.h>

#undef stderr
#undef stdin
#undef stdout

extern FILE __sF[];

FILE* stderr = &__sF[2];
FILE* stdin = &__sF[0];
FILE* stdout = &__sF[1];

FILE* fopen64(const char* path, const char* mode) {
    return fopen(path, mode);
}

int fseeko64(FILE* stream, off64_t offset, int whence) {
    return fseeko(stream, (off_t)offset, whence);
}

off64_t ftello64(FILE* stream) {
    return (off64_t)ftello(stream);
}

size_t __fread_chk(void* buf, size_t size, size_t count, FILE* stream, size_t buf_size) {
    (void)buf_size;
    return fread(buf, size, count, stream);
}

size_t __fwrite_chk(const void* buf, size_t size, size_t count, FILE* stream, size_t buf_size) {
    (void)buf_size;
    return fwrite(buf, size, count, stream);
}

typedef void* iconv_t;

iconv_t iconv_open(const char* tocode, const char* fromcode) {
    (void)tocode;
    (void)fromcode;
    errno = EINVAL;
    return (iconv_t)-1;
}

size_t iconv(iconv_t cd, char** inbuf, size_t* inbytesleft, char** outbuf, size_t* outbytesleft) {
    (void)cd;
    (void)inbuf;
    (void)inbytesleft;
    (void)outbuf;
    (void)outbytesleft;
    errno = EINVAL;
    return (size_t)-1;
}

int iconv_close(iconv_t cd) {
    (void)cd;
    return 0;
}
#endif
