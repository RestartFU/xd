// WebKitGTK's production builds hardcode their auxiliary executable directory.
// Redirect only those executable arguments through GLib's public launch API.
// Linking this into xd (instead of LD_PRELOAD) keeps host tools unaffected.
#define _GNU_SOURCE
#include <dlfcn.h>
#include <gio/gio.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

// GnuTLS's system trust path belongs to the build distribution, whereas the
// bundle's CA file is relocatable. SSL_CERT_FILE only configures OpenSSL.
// Initialize GIO before WebKit creates its network sessions; setting the backend
// database also covers calls made internally by GIO's TLS connections.
__attribute__((constructor))
static void configure_bundle_tls(void)
{
    const char *root = g_getenv("XD_BROWSER_BUNDLE_ROOT");
    if (!root || !*root)
        return;
    gchar *path = g_build_filename(root, "etc", "ssl", "certs",
                                  "ca-certificates.crt", NULL);
    GError *error = NULL;
    GTlsDatabase *database = g_tls_file_database_new(path, &error);
    if (!database)
        g_error("Cannot load browser certificate authorities: %s",
                error ? error->message : "unknown error");
    g_tls_backend_set_default_database(g_tls_backend_get_default(), database);
    g_object_unref(database);
    g_free(path);
}

static const char *webkit_helper(const char *path)
{
    const char *prefix = "/usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/";
    if (strncmp(path, prefix, strlen(prefix)) != 0)
        return NULL;
    const char *name = path + strlen(prefix);
    return strcmp(name, "WebKitWebProcess") == 0 ||
           strcmp(name, "WebKitNetworkProcess") == 0 ||
           strcmp(name, "WebKitGPUProcess") == 0 ? name : NULL;
}

GSubprocess *g_subprocess_launcher_spawnv(GSubprocessLauncher *launcher,
                                         const gchar *const *argv,
                                         GError **error)
{
    typedef GSubprocess *(*Spawn)(GSubprocessLauncher *, const gchar *const *, GError **);
    Spawn spawn = (Spawn)dlsym(RTLD_NEXT, "g_subprocess_launcher_spawnv");
    const char *root = getenv("XD_BROWSER_BUNDLE_ROOT");
    if (!root || !*root || !argv || !argv[0])
        return spawn(launcher, argv, error);

    size_t executable = 0;
    // WebKit passes its sandbox policy through --args FD, then the executable.
    // Leave that policy and the host's bubblewrap/dbus-proxy paths untouched.
    if (strcmp(argv[0], "/usr/bin/bwrap") == 0 && argv[1] &&
        strcmp(argv[1], "--args") == 0 && argv[2] && argv[3] &&
        strcmp(argv[3], "--") == 0 && argv[4])
        executable = 4;
    const char *helper = webkit_helper(argv[executable]);
    if (!helper)
        return spawn(launcher, argv, error);

    size_t count = 0;
    while (argv[count])
        count++;
    const char **relocated = malloc((count + 1) * sizeof(*relocated));
    char *path = NULL;
    if (!relocated || asprintf(&path, "%s/libexec/webkit2gtk-4.1/%s", root, helper) < 0) {
        free(relocated);
        g_set_error_literal(error, G_SPAWN_ERROR, G_SPAWN_ERROR_NOMEM,
                            "Cannot allocate WebKit helper path");
        return NULL;
    }
    memcpy(relocated, argv, (count + 1) * sizeof(*relocated));
    relocated[executable] = path;
    GSubprocess *process = spawn(launcher, relocated, error);
    free(path);
    free(relocated);
    return process;
}
