// Exercise the same default TLS database used by WebKit's network sessions.
#include <gio/gio.h>

int main(int argc, char **argv)
{
    g_assert_cmpint(argc, ==, 4);
    GError *error = NULL;
    GTlsDatabase *database = g_tls_backend_get_default_database(g_tls_backend_get_default());
    g_assert_true(G_IS_TLS_FILE_DATABASE(database));
    GTlsCertificate *certificate = g_tls_certificate_new_from_file(argv[1], &error);
    g_assert_no_error(error);
    GSocketConnectable *identity = g_network_address_new(argv[2], 443);
    GTlsCertificateFlags flags = g_tls_database_verify_chain(
        database, certificate, G_TLS_DATABASE_PURPOSE_AUTHENTICATE_SERVER,
        identity, NULL, G_TLS_DATABASE_VERIFY_NONE, NULL, &error);
    g_assert_no_error(error);
    g_assert_cmpuint(flags, ==, (guint)g_ascii_strtoull(argv[3], NULL, 10));
    g_object_unref(identity);
    g_object_unref(certificate);
    return 0;
}
