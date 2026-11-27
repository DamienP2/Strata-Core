// Le pont du Core vers Intel Media Transport Library (MTL) : ST 2110-20 (vidéo
// non compressée, en v210) et ST 2110-30 (son PCM) reçus et envoyés sur les
// ports média (E810 en DPDK, AF_XDP ou socket du noyau).
//
// libmtl.so est chargée au lancement (dlopen) : le Core démarre sans elle. Les
// en-têtes de MTL (vendored, include/) donnent les types ; seules les
// fonctions sont prises dans la bibliothèque.
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "st30_pipeline_api.h"
#include "st_pipeline_api.h"

static void *lib;

#define F(name) static __typeof__(&name) p_##name
F(mtl_init);
F(mtl_uninit);
F(st20p_rx_create);
F(st20p_rx_get_frame);
F(st20p_rx_put_frame);
F(st20p_rx_free);
F(st20p_rx_get_session_stats);
F(st20p_rx_set_block_timeout);
F(st20p_tx_create);
F(st20p_tx_get_frame);
F(st20p_tx_put_frame);
F(st20p_tx_free);
F(st20p_tx_set_block_timeout);
F(st30p_rx_create);
F(st30p_rx_get_frame);
F(st30p_rx_put_frame);
F(st30p_rx_free);
F(st30p_rx_get_session_stats);
F(st30p_rx_set_block_timeout);
F(st30p_tx_create);
F(st30p_tx_get_frame);
F(st30p_tx_put_frame);
F(st30p_tx_free);
F(st30p_tx_set_block_timeout);
F(st30_calculate_framebuff_size);
#undef F
// Facultative : les compteurs d'un port (le débit mesuré, surtout en DPDK).
static __typeof__(&mtl_get_port_stats) p_mtl_get_port_stats;

static void set_err(char *err, int len, const char *msg) {
    if (err && len > 0) {
        snprintf(err, (size_t)len, "%s", msg);
    }
}

#define SYM(name)                                                                \
    do {                                                                         \
        p_##name = (__typeof__(p_##name))dlsym(lib, #name);                      \
        if (!p_##name) {                                                         \
            snprintf(err, (size_t)errlen, "libmtl has no %s (another version?)", \
                     #name);                                                     \
            return -1;                                                           \
        }                                                                        \
    } while (0)

// Charge libmtl (`path`). 0, ou -1 avec `err`.
int smtl_load(const char *path, char *err, int errlen) {
    if (lib) {
        return 0;
    }
    void *l = dlopen(path, RTLD_NOW | RTLD_GLOBAL);
    if (!l) {
        set_err(err, errlen, dlerror());
        return -1;
    }
    lib = l;
    SYM(mtl_init);
    SYM(mtl_uninit);
    SYM(st20p_rx_create);
    SYM(st20p_rx_get_frame);
    SYM(st20p_rx_put_frame);
    SYM(st20p_rx_free);
    SYM(st20p_rx_get_session_stats);
    SYM(st20p_rx_set_block_timeout);
    SYM(st20p_tx_create);
    SYM(st20p_tx_get_frame);
    SYM(st20p_tx_put_frame);
    SYM(st20p_tx_free);
    SYM(st20p_tx_set_block_timeout);
    SYM(st30p_rx_create);
    SYM(st30p_rx_get_frame);
    SYM(st30p_rx_put_frame);
    SYM(st30p_rx_free);
    SYM(st30p_rx_get_session_stats);
    SYM(st30p_rx_set_block_timeout);
    SYM(st30p_tx_create);
    SYM(st30p_tx_get_frame);
    SYM(st30p_tx_put_frame);
    SYM(st30p_tx_free);
    SYM(st30p_tx_set_block_timeout);
    SYM(st30_calculate_framebuff_size);
    p_mtl_get_port_stats = (__typeof__(p_mtl_get_port_stats))dlsym(lib, "mtl_get_port_stats");
    return 0;
}

// Un port média : son nom pour MTL (« 0000:3b:00.0 » en DPDK,
// « native_af_xdp:ens2f0 », « kernel:ens2f0 »), son mode, son adresse.
typedef struct {
    char name[64];
    int pmd; // enum mtl_pmd_type
    uint8_t ip[4];
    uint8_t mask[4];
    uint8_t gateway[4];
} smtl_port;

typedef struct {
    uint32_t width, height;
    int fps; // enum st_fps
    int interlaced;
} smtl_video;

// Une image (une trame en entrelacé) prise à MTL.
typedef struct {
    void *frame;
    uint8_t *data;
    uint64_t linesize;
    uint32_t width, height;
    int second_field;
    int complete;
    uint64_t timestamp;
} smtl_frame;

// L'heure TAI du serveur (tenue par PTP/NTP) : CLOCK_REALTIME + décalage.
static uint64_t tai_offset_ns;
static uint64_t system_tai(void *priv) {
    (void)priv;
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    return (uint64_t)ts.tv_sec * 1000000000ull + (uint64_t)ts.tv_nsec + tai_offset_ns;
}

typedef struct {
    mtl_handle mt;
    char ports[MTL_PORT_MAX][MTL_PORT_MAX_LEN];
    int num_ports;
} smtl;

// Ouvre MTL sur `n` ports (1, ou 2 : rouge et bleu en 2022-7). `lcores` :
// les cœurs de ses fils (« 2,3 »), ou NULL.
void *smtl_open(const smtl_port *ports, int n, uint32_t tai_offset, const char *lcores, char *err, int errlen) {
    if (!lib) {
        set_err(err, errlen, "libmtl is not loaded");
        return NULL;
    }
    if (n < 1 || n > MTL_PORT_MAX) {
        set_err(err, errlen, "1 or 2 media ports");
        return NULL;
    }
    tai_offset_ns = (uint64_t)tai_offset * 1000000000ull;
    static struct mtl_init_params p;
    memset(&p, 0, sizeof(p));
    smtl *s = (smtl *)calloc(1, sizeof(smtl));
    for (int i = 0; i < n; i++) {
        snprintf(p.port[i], sizeof(p.port[i]), "%s", ports[i].name);
        snprintf(s->ports[i], sizeof(s->ports[i]), "%s", ports[i].name);
        p.pmd[i] = (enum mtl_pmd_type)ports[i].pmd;
        memcpy(p.sip_addr[i], ports[i].ip, 4);
        memcpy(p.netmask[i], ports[i].mask, 4);
        memcpy(p.gateway[i], ports[i].gateway, 4);
        p.tx_queues_cnt[i] = 16;
        p.rx_queues_cnt[i] = 16;
    }
    p.num_ports = (uint8_t)n;
    s->num_ports = n;
    // Ses fils dorment quand ils n'ont rien à faire, au lieu de tourner à vide (un cœur pris en
    // permanence) ; sous charge (de la vidéo à plein débit), ils ne s'arrêtent pas.
    p.flags = MTL_FLAG_DEV_AUTO_START_STOP | MTL_FLAG_BIND_NUMA | MTL_FLAG_TASKLET_SLEEP;
    p.log_level = MTL_LOG_LEVEL_ERR;
    p.ptp_get_time_fn = system_tai;
    p.lcores = (char *)lcores;
    s->mt = p_mtl_init(&p);
    if (!s->mt) {
        set_err(err, errlen,
                "cannot start MTL on the media ports (hugepages, port names and addresses, "
                "rights?)");
        free(s);
        return NULL;
    }
    return s;
}

// Les compteurs du port `port` (0 ou 1) : octets reçus, envoyés, paquets perdus par la
// carte. 0, ou -1 sans eux.
int smtl_port_stats(void *h, int port, uint64_t *rx_bytes, uint64_t *tx_bytes, uint64_t *rx_dropped) {
    smtl *s = (smtl *)h;
    struct mtl_port_status st;
    if (!s || !p_mtl_get_port_stats) {
        return -1;
    }
    memset(&st, 0, sizeof(st));
    if (p_mtl_get_port_stats(s->mt, (enum mtl_port)port, &st) != 0) {
        return -1;
    }
    *rx_bytes = st.rx_bytes;
    *tx_bytes = st.tx_bytes;
    *rx_dropped = st.rx_hw_dropped_packets;
    return 0;
}

void smtl_close(void *h) {
    smtl *s = (smtl *)h;
    if (s) {
        p_mtl_uninit(s->mt);
        free(s);
    }
}

// Reçoit un flux vidéo : `n` pattes (rouge, et bleue en 2022-7) ;
// `port_index[i]` : le port média de chaque patte.
void *smtl_rx_open(void *h, const char *name, int n, const int *port_index, const uint8_t (*ip)[4],
                   const uint16_t *udp, uint8_t pt, smtl_video v, char *err, int errlen) {
    smtl *s = (smtl *)h;
    struct st20p_rx_ops ops;
    memset(&ops, 0, sizeof(ops));
    ops.name = name;
    ops.port.num_port = (uint8_t)n;
    for (int i = 0; i < n; i++) {
        snprintf(ops.port.port[i], sizeof(ops.port.port[i]), "%s", s->ports[port_index[i]]);
        memcpy(ops.port.ip_addr[i], ip[i], 4);
        ops.port.udp_port[i] = udp[i];
    }
    ops.port.payload_type = pt;
    ops.width = v.width;
    ops.height = v.height;
    ops.fps = (enum st_fps)v.fps;
    ops.interlaced = v.interlaced != 0;
    ops.transport_fmt = ST20_FMT_YUV_422_10BIT;
    ops.output_fmt = ST_FRAME_FMT_V210;
    ops.device = ST_PLUGIN_DEVICE_AUTO;
    ops.framebuff_cnt = 3;
    ops.flags = ST20P_RX_FLAG_BLOCK_GET;
    st20p_rx_handle rx = p_st20p_rx_create(s->mt, &ops);
    if (!rx) {
        set_err(err, errlen, "cannot receive this ST 2110 flow (address, format?)");
        return NULL;
    }
    p_st20p_rx_set_block_timeout(rx, 200ull * 1000000ull);
    return rx;
}

// La prochaine image reçue (attend 200 ms au plus) : 1, ou 0.
int smtl_rx_get(void *rx, smtl_frame *out) {
    struct st_frame *f = p_st20p_rx_get_frame((st20p_rx_handle)rx);
    if (!f) {
        return 0;
    }
    out->frame = f;
    out->data = (uint8_t *)f->addr[0];
    out->linesize = f->linesize[0];
    out->width = f->width;
    out->height = f->height;
    out->second_field = f->second_field;
    out->complete = f->status == ST_FRAME_STATUS_COMPLETE || f->status == ST_FRAME_STATUS_RECONSTRUCTED;
    out->timestamp = f->timestamp;
    return 1;
}

void smtl_rx_put(void *rx, void *frame) {
    p_st20p_rx_put_frame((st20p_rx_handle)rx, (struct st_frame *)frame);
}

// Paquets reçus et perdus depuis l'ouverture.
void smtl_rx_stats(void *rx, uint64_t *received, uint64_t *lost) {
    struct st20_rx_user_stats st;
    memset(&st, 0, sizeof(st));
    if (p_st20p_rx_get_session_stats((st20p_rx_handle)rx, &st) == 0) {
        *received = st.common.stat_pkts_received;
        *lost = st.common.stat_lost_packets;
    }
}

void smtl_rx_close(void *rx) {
    p_st20p_rx_free((st20p_rx_handle)rx);
}

// Envoie un flux vidéo : `n` pattes, vers `ip` / `udp`.
void *smtl_tx_open(void *h, const char *name, int n, const int *port_index, const uint8_t (*ip)[4],
                   const uint16_t *udp, uint8_t pt, smtl_video v, char *err, int errlen) {
    smtl *s = (smtl *)h;
    struct st20p_tx_ops ops;
    memset(&ops, 0, sizeof(ops));
    ops.name = name;
    ops.port.num_port = (uint8_t)n;
    for (int i = 0; i < n; i++) {
        snprintf(ops.port.port[i], sizeof(ops.port.port[i]), "%s", s->ports[port_index[i]]);
        memcpy(ops.port.dip_addr[i], ip[i], 4);
        ops.port.udp_port[i] = udp[i];
    }
    ops.port.payload_type = pt;
    ops.width = v.width;
    ops.height = v.height;
    ops.fps = (enum st_fps)v.fps;
    ops.interlaced = v.interlaced != 0;
    ops.input_fmt = ST_FRAME_FMT_V210;
    ops.transport_fmt = ST20_FMT_YUV_422_10BIT;
    ops.transport_pacing = ST21_PACING_NARROW;
    ops.transport_packing = ST20_PACKING_BPM;
    ops.device = ST_PLUGIN_DEVICE_AUTO;
    ops.framebuff_cnt = 3;
    ops.flags = ST20P_TX_FLAG_BLOCK_GET;
    st20p_tx_handle tx = p_st20p_tx_create(s->mt, &ops);
    if (!tx) {
        set_err(err, errlen, "cannot send this ST 2110 flow (address, format?)");
        return NULL;
    }
    p_st20p_tx_set_block_timeout(tx, 200ull * 1000000ull);
    return tx;
}

// Une image libre à remplir (attend 200 ms au plus) : 1, ou 0.
int smtl_tx_get(void *tx, smtl_frame *out) {
    struct st_frame *f = p_st20p_tx_get_frame((st20p_tx_handle)tx);
    if (!f) {
        return 0;
    }
    out->frame = f;
    out->data = (uint8_t *)f->addr[0];
    out->linesize = f->linesize[0];
    out->width = f->width;
    out->height = f->height;
    out->second_field = f->second_field;
    out->complete = 1;
    out->timestamp = 0;
    return 1;
}

// Envoie l'image remplie ; en entrelacé, `second_field` dit quelle trame.
int smtl_tx_put(void *tx, void *frame, int second_field) {
    struct st_frame *f = (struct st_frame *)frame;
    f->second_field = second_field != 0;
    return p_st20p_tx_put_frame((st20p_tx_handle)tx, f);
}

void smtl_tx_close(void *tx) {
    p_st20p_tx_free((st20p_tx_handle)tx);
}

// ---------- Le son : ST 2110-30 ----------

// Un flux audio : canaux, format (enum st30_fmt : 1 L16, 2 L24), durée des
// paquets (enum st30_ptime : 0 pour 1 ms), 48 kHz ; `frame_ms` : la durée d'un
// bloc échangé avec le Core.
typedef struct {
    uint16_t channels;
    int fmt;
    int ptime;
    uint32_t frame_ms;
} smtl_audio;

// Un bloc de son : les échantillons tels que sur le réseau (gros-boutiens,
// entrelacés).
typedef struct {
    void *frame;
    uint8_t *data;
    uint64_t size;
    uint64_t timestamp;
    int complete;
} smtl_aframe;

static int audio_frame_size(smtl_audio a) {
    return p_st30_calculate_framebuff_size((enum st30_fmt)a.fmt, (enum st30_ptime)a.ptime, ST30_SAMPLING_48K,
                                           a.channels, (uint64_t)a.frame_ms * 1000000ull, NULL);
}

// Reçoit un flux audio : `n` pattes (2022-7 comme la vidéo).
void *smtl_arx_open(void *h, const char *name, int n, const int *port_index, const uint8_t (*ip)[4],
                    const uint16_t *udp, uint8_t pt, smtl_audio a, char *err, int errlen) {
    smtl *s = (smtl *)h;
    int size = audio_frame_size(a);
    if (size <= 0) {
        set_err(err, errlen, "unsupported audio format (channels, sample size, packet time)");
        return NULL;
    }
    struct st30p_rx_ops ops;
    memset(&ops, 0, sizeof(ops));
    ops.name = name;
    ops.port.num_port = (uint8_t)n;
    for (int i = 0; i < n; i++) {
        snprintf(ops.port.port[i], sizeof(ops.port.port[i]), "%s", s->ports[port_index[i]]);
        memcpy(ops.port.ip_addr[i], ip[i], 4);
        ops.port.udp_port[i] = udp[i];
    }
    ops.port.payload_type = pt;
    ops.fmt = (enum st30_fmt)a.fmt;
    ops.channel = a.channels;
    ops.sampling = ST30_SAMPLING_48K;
    ops.ptime = (enum st30_ptime)a.ptime;
    ops.framebuff_cnt = 4;
    ops.framebuff_size = (uint32_t)size;
    ops.flags = ST30P_RX_FLAG_BLOCK_GET;
    st30p_rx_handle rx = p_st30p_rx_create(s->mt, &ops);
    if (!rx) {
        set_err(err, errlen, "cannot receive this ST 2110-30 flow (address, format?)");
        return NULL;
    }
    p_st30p_rx_set_block_timeout(rx, 200ull * 1000000ull);
    return rx;
}

// Le prochain bloc reçu (attend 200 ms au plus) : 1, ou 0.
int smtl_arx_get(void *rx, smtl_aframe *out) {
    struct st30_frame *f = p_st30p_rx_get_frame((st30p_rx_handle)rx);
    if (!f) {
        return 0;
    }
    out->frame = f;
    out->data = (uint8_t *)f->addr;
    out->size = f->data_size ? f->data_size : f->buffer_size;
    out->timestamp = f->timestamp;
    out->complete = 1;
    return 1;
}

void smtl_arx_put(void *rx, void *frame) {
    p_st30p_rx_put_frame((st30p_rx_handle)rx, (struct st30_frame *)frame);
}

void smtl_arx_stats(void *rx, uint64_t *received, uint64_t *lost) {
    struct st30_rx_user_stats st;
    memset(&st, 0, sizeof(st));
    if (p_st30p_rx_get_session_stats((st30p_rx_handle)rx, &st) == 0) {
        *received = st.common.stat_pkts_received;
        *lost = st.common.stat_lost_packets;
    }
}

void smtl_arx_close(void *rx) {
    p_st30p_rx_free((st30p_rx_handle)rx);
}

// Envoie un flux audio : `n` pattes, vers `ip` / `udp`.
void *smtl_atx_open(void *h, const char *name, int n, const int *port_index, const uint8_t (*ip)[4],
                    const uint16_t *udp, uint8_t pt, smtl_audio a, char *err, int errlen) {
    smtl *s = (smtl *)h;
    int size = audio_frame_size(a);
    if (size <= 0) {
        set_err(err, errlen, "unsupported audio format (channels, sample size, packet time)");
        return NULL;
    }
    struct st30p_tx_ops ops;
    memset(&ops, 0, sizeof(ops));
    ops.name = name;
    ops.port.num_port = (uint8_t)n;
    for (int i = 0; i < n; i++) {
        snprintf(ops.port.port[i], sizeof(ops.port.port[i]), "%s", s->ports[port_index[i]]);
        memcpy(ops.port.dip_addr[i], ip[i], 4);
        ops.port.udp_port[i] = udp[i];
    }
    ops.port.payload_type = pt;
    ops.fmt = (enum st30_fmt)a.fmt;
    ops.channel = a.channels;
    ops.sampling = ST30_SAMPLING_48K;
    ops.ptime = (enum st30_ptime)a.ptime;
    ops.framebuff_cnt = 4;
    ops.framebuff_size = (uint32_t)size;
    ops.flags = ST30P_TX_FLAG_BLOCK_GET;
    st30p_tx_handle tx = p_st30p_tx_create(s->mt, &ops);
    if (!tx) {
        set_err(err, errlen, "cannot send this ST 2110-30 flow (address, format?)");
        return NULL;
    }
    p_st30p_tx_set_block_timeout(tx, 200ull * 1000000ull);
    return tx;
}

// Un bloc libre à remplir (attend 200 ms au plus) : 1, ou 0.
int smtl_atx_get(void *tx, smtl_aframe *out) {
    struct st30_frame *f = p_st30p_tx_get_frame((st30p_tx_handle)tx);
    if (!f) {
        return 0;
    }
    out->frame = f;
    out->data = (uint8_t *)f->addr;
    out->size = f->buffer_size;
    out->timestamp = 0;
    out->complete = 1;
    return 1;
}

int smtl_atx_put(void *tx, void *frame) {
    return p_st30p_tx_put_frame((st30p_tx_handle)tx, (struct st30_frame *)frame);
}

void smtl_atx_close(void *tx) {
    p_st30p_tx_free((st30p_tx_handle)tx);
}
