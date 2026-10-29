// Tauri's iPhone layer calls functions that Swift code normally provides:
// its own (plugins written in Swift, stdout to the system log) and those of
// swift-rs, the bridge it passes strings over. Kagaz has no iPhone plugins,
// so these stand in for them in plain C: no Swift, and so no Xcode, is
// needed to build the app. A plugin call is answered at once with an
// error, which Kagaz's Rust side already treats as "not available on this
// phone".

#include <stdint.h>
#include <stdlib.h>

typedef void (*message_callback)(int id, int success, const char *payload);
typedef void (*channel_callback)(uint64_t id, const char *payload);

void log_stdout(void) {}

void register_plugin(const void *name, const void *plugin, const void *config, const void *webview) {
    (void)name;
    (void)plugin;
    (void)config;
    (void)webview;
}

void on_webview_created(const void *webview, const void *controller) {
    (void)webview;
    (void)controller;
}

void run_plugin_command(int id, const void *name, const void *method, const void *data,
                        message_callback callback, channel_callback send_channel_data) {
    (void)name;
    (void)method;
    (void)data;
    (void)send_channel_data;
    callback(id, 0, "\"not available on iPhone yet\"");
}

// The file dialog plugin's iPhone half; none here (its calls are refused).
const void *init_plugin_dialog(void) {
    return 0;
}

// swift-rs's strings and data: Rust makes them only to hand them to the
// functions above, which never look inside, so a counted placeholder will
// do. Anything else handed to retain or release (an object from UIKit) is
// left alone: only placeholders carry the mark.
#define MARK 0x4b6167617aUL

struct placeholder {
    unsigned long mark;
    long refs;
};

static void *placeholder(void) {
    struct placeholder *p = malloc(sizeof *p);
    if (p) {
        p->mark = MARK;
        p->refs = 1;
    }
    return p;
}

void *string_from_bytes(const uint8_t *data, long size) {
    (void)data;
    (void)size;
    return placeholder();
}

void *data_from_bytes(const uint8_t *data, long size) {
    (void)data;
    (void)size;
    return placeholder();
}

void retain_object(void *object) {
    struct placeholder *p = object;
    if (p && p->mark == MARK) __atomic_add_fetch(&p->refs, 1, __ATOMIC_RELAXED);
}

void release_object(void *object) {
    struct placeholder *p = object;
    if (p && p->mark == MARK && __atomic_sub_fetch(&p->refs, 1, __ATOMIC_ACQ_REL) == 0) {
        p->mark = 0;
        free(p);
    }
}
