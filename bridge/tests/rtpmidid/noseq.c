/*
 * An ALSA sequencer that is always empty, preloaded into rtpmidid so that
 * it starts without /dev/snd/seq: rtpmidid opens the sequencer at start
 * even when it only bridges a raw MIDI device. Every call that takes the
 * sequencer handle succeeds and does nothing; no event ever arrives.
 */

#include <alsa/asoundlib.h>
#include <errno.h>
#include <unistd.h>

static int never[2] = {-1, -1};
static int next_port = 0;

int snd_seq_open(snd_seq_t **handle, const char *name, int streams, int mode) {
  (void)name, (void)streams, (void)mode;
  if (never[0] < 0 && pipe(never) < 0)
    return -errno;
  *handle = (snd_seq_t *)never;
  return 0;
}

int snd_seq_close(snd_seq_t *s) { (void)s; return 0; }
int snd_seq_set_client_name(snd_seq_t *s, const char *n) { (void)s, (void)n; return 0; }
int snd_seq_get_client_pool(snd_seq_t *s, snd_seq_client_pool_t *p) { (void)s, (void)p; return -ENOSYS; }
int snd_seq_set_client_pool(snd_seq_t *s, snd_seq_client_pool_t *p) { (void)s, (void)p; return -ENOSYS; }
int snd_seq_set_input_buffer_size(snd_seq_t *s, size_t n) { (void)s, (void)n; return 0; }
int snd_seq_set_output_buffer_size(snd_seq_t *s, size_t n) { (void)s, (void)n; return 0; }

int snd_seq_get_client_info(snd_seq_t *s, snd_seq_client_info_t *info) {
  (void)s;
  snd_seq_client_info_set_client(info, 128);
  return 0;
}

int snd_seq_poll_descriptors_count(snd_seq_t *s, short events) { (void)s, (void)events; return 1; }

int snd_seq_poll_descriptors(snd_seq_t *s, struct pollfd *pfds, unsigned int space, short events) {
  (void)s;
  if (space < 1)
    return 0;
  pfds[0].fd = never[0];
  pfds[0].events = events;
  pfds[0].revents = 0;
  return 1;
}

int snd_seq_event_input(snd_seq_t *s, snd_seq_event_t **ev) { (void)s, (void)ev; return -EAGAIN; }
int snd_seq_event_input_pending(snd_seq_t *s, int f) { (void)s, (void)f; return 0; }
int snd_seq_event_output(snd_seq_t *s, snd_seq_event_t *ev) { (void)s, (void)ev; return 0; }
int snd_seq_drain_output(snd_seq_t *s) { (void)s; return 0; }
int snd_seq_drop_output(snd_seq_t *s) { (void)s; return 0; }
int snd_seq_drop_input(snd_seq_t *s) { (void)s; return 0; }

int snd_seq_create_simple_port(snd_seq_t *s, const char *n, unsigned int caps, unsigned int type) {
  (void)s, (void)n, (void)caps, (void)type;
  return next_port++;
}

int snd_seq_delete_port(snd_seq_t *s, int port) { (void)s, (void)port; return 0; }
int snd_seq_connect_from(snd_seq_t *s, int m, int c, int p) { (void)s, (void)m, (void)c, (void)p; return 0; }
int snd_seq_connect_to(snd_seq_t *s, int m, int c, int p) { (void)s, (void)m, (void)c, (void)p; return 0; }
int snd_seq_disconnect_from(snd_seq_t *s, int m, int c, int p) { (void)s, (void)m, (void)c, (void)p; return 0; }
int snd_seq_disconnect_to(snd_seq_t *s, int m, int c, int p) { (void)s, (void)m, (void)c, (void)p; return 0; }
int snd_seq_get_any_client_info(snd_seq_t *s, int c, snd_seq_client_info_t *i) { (void)s, (void)c, (void)i; return -ENOENT; }
int snd_seq_get_any_port_info(snd_seq_t *s, int c, int p, snd_seq_port_info_t *i) { (void)s, (void)c, (void)p, (void)i; return -ENOENT; }
int snd_seq_get_port_info(snd_seq_t *s, int p, snd_seq_port_info_t *i) { (void)s, (void)p, (void)i; return -ENOENT; }
int snd_seq_query_next_client(snd_seq_t *s, snd_seq_client_info_t *i) { (void)s, (void)i; return -ENOENT; }
int snd_seq_query_next_port(snd_seq_t *s, snd_seq_port_info_t *i) { (void)s, (void)i; return -ENOENT; }
int snd_seq_query_port_subscribers(snd_seq_t *s, snd_seq_query_subscribe_t *q) { (void)s, (void)q; return -ENOENT; }
int snd_seq_unsubscribe_port(snd_seq_t *s, snd_seq_port_subscribe_t *p) { (void)s, (void)p; return 0; }
