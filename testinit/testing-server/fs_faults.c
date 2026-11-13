// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#define _GNU_SOURCE
#include <dirent.h>
#include <dlfcn.h>
#include <errno.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static void *next_symbol(const char *name) {
    int saved_errno = errno;
    void *symbol = dlsym(RTLD_NEXT, name);
    if (!symbol) {
        fprintf(stderr, "Cannot resolve %s: %s\n", name, dlerror());
        _Exit(127);
    }
    errno = saved_errno;
    return symbol;
}

static bool enabled(const char *scenario) {
    int saved_errno = errno;
    const char *current = getenv("TEST_SCENARIO");
    bool result = current && strcmp(current, scenario) == 0 &&
                  access("/test/isolated-provisioning", F_OK) == 0;
    errno = saved_errno;
    return result;
}

static bool is_descriptor_path(int fd, const char *expected) {
    int saved_errno = errno;
    char descriptor[64], path[256];
    snprintf(descriptor, sizeof(descriptor), "/proc/self/fd/%d", fd);
    ssize_t length = readlink(descriptor, path, sizeof(path));
    errno = saved_errno;
    return length >= 0 && (size_t)length == strlen(expected) &&
           memcmp(path, expected, (size_t)length) == 0;
}

// Inject faults only at the disposable fixture's paths.
ssize_t write(int fd, const void *buffer, size_t count) {
    if ((enabled("sshd-create-write-error") ||
         enabled("sshd-append-write-error")) &&
        is_descriptor_path(fd, "/etc/ssh/sshd_config")) {
        errno = ENOSPC;
        return -1;
    }
    ssize_t (*write_next)(int, const void *, size_t) =
        (ssize_t (*)(int, const void *, size_t))next_symbol("write");
    return write_next(fd, buffer, count);
}

int chmod(const char *path, mode_t mode) {
    if (enabled("state-permission-error") &&
        strcmp(path, "/work/state") == 0 && mode == 0700) {
        errno = EPERM;
        return -1;
    }
    int (*set_mode)(const char *, mode_t) =
        (int (*)(const char *, mode_t))next_symbol("chmod");
    return set_mode(path, mode);
}

struct dirent64 *readdir64(DIR *directory) {
    if (enabled("clean-read-error") &&
        is_descriptor_path(dirfd(directory), "/work/state")) {
        errno = EIO;
        return NULL;
    }
    struct dirent64 *(*read_next)(DIR *) =
        (struct dirent64 *(*)(DIR *))next_symbol("readdir64");
    return read_next(directory);
}

int unlink(const char *path) {
    int (*remove_file)(const char *) =
        (int (*)(const char *))next_symbol("unlink");
    int result = remove_file(path);
    if (result == 0 && enabled("clean-race") &&
        strcmp(path, "/work/state/vanished.provisioned") == 0) {
        errno = ENOENT;
        return -1;
    }
    return result;
}
