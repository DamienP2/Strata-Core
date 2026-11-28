// Le pont du Core vers Intel Media Transport Library (MTL) : ST 2110-20 (vidéo
// non compressée, en v210), ST 2110-22 (vidéo compressée JPEG XS, par le greffon
// SVT-JPEG-XS de MTL), ST 2110-30 (son PCM) et ST 2110-40 (données annexes)
// reçus et envoyés sur les ports média (E810 en DPDK, AF_XDP ou socket du noyau).
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
#include "st40_pipeline_api.h"
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
F(st40p_rx_create);
F(st40p_rx_get_frame);
F(st40p_rx_put_frame);
F(st40p_rx_free);
F(st40p_rx_get_session_stats);
F(st40p_rx_set_block_timeout);
F(st40p_tx_create);
F(st40p_tx_get_frame);
F(st40p_tx_put_frame);
F(st40p_tx_free);
F(st40p_tx_set_block_timeout);
F(st22p_rx_create);
F(st22p_rx_get_frame);
F(st22p_rx_put_frame);
F(st22p_rx_free);
F(st22p_rx_set_block_timeout);
F(st22p_tx_create);
F(st22p_tx_get_frame);
F(st22p_tx_put_frame);
F(st22p_tx_free);
F(st22p_tx_set_block_timeout);
#undef F
// Les données (st40p) sont facultatives : une bibliothèque sans elles garde la
// vidéo et le son.
static int has_st40p;
// Le ST 2110-22 (vidéo compressée) aussi.
static int has_st22p;
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
            goto fail;                                                           \
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
#define OPT(name) p_##name = (__typeof__(p_##name))dlsym(lib, #name)
    OPT(st40p_rx_create);
    OPT(st40p_rx_get_frame);
    OPT(st40p_rx_put_frame);
    OPT(st40p_rx_free);
    OPT(st40p_rx_get_session_stats);
    OPT(st40p_rx_set_block_timeout);
    OPT(st40p_tx_create);
    OPT(st40p_tx_get_frame);
    OPT(st40p_tx_put_frame);
    OPT(st40p_tx_free);
    OPT(st40p_tx_set_block_timeout);
    OPT(st22p_rx_create);
    OPT(st22p_rx_get_frame);
    OPT(st22p_rx_put_frame);
    OPT(st22p_rx_free);
    OPT(st22p_rx_set_block_timeout);
    OPT(st22p_tx_create);
    OPT(st22p_tx_get_frame);
    OPT(st22p_tx_put_frame);
    OPT(st22p_tx_free);
    OPT(st22p_tx_set_block_timeout);
#undef OPT
    has_st22p = p_st22p_rx_create && p_st22p_rx_get_frame && p_st22p_rx_put_frame && p_st22p_rx_free &&
                p_st22p_rx_set_block_timeout && p_st22p_tx_create && p_st22p_tx_get_frame && p_st22p_tx_put_frame &&
                p_st22p_tx_free && p_st22p_tx_set_block_timeout;
    has_st40p = p_st40p_rx_create && p_st40p_rx_get_frame && p_st40p_rx_put_frame && p_st40p_rx_free &&
                p_st40p_rx_get_session_stats && p_st40p_rx_set_block_timeout && p_st40p_tx_create &&
                p_st40p_tx_get_frame && p_st40p_tx_put_frame && p_st40p_tx_free && p_st40p_tx_set_block_timeout;
    return 0;
fail:
    // Une version qui ne convient pas : refermée, et la suivante essayée en entier (aucune
    // fonction de celle-ci ne sera appelée).
    dlclose(l);
    lib = NULL;
    p_mtl_get_port_stats = NULL;
    return -1;
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
                   const uint16_t *udp, const uint8_t (*sip)[4], uint8_t pt, smtl_video v, char *err, int errlen) {
    smtl *s = (smtl *)h;
    struct st20p_rx_ops ops;
    memset(&ops, 0, sizeof(ops));
    ops.name = name;
    ops.port.num_port = (uint8_t)n;
    for (int i = 0; i < n; i++) {
        snprintf(ops.port.port[i], sizeof(ops.port.port[i]), "%s", s->ports[port_index[i]]);
        memcpy(ops.port.ip_addr[i], ip[i], 4);
        ops.port.udp_port[i] = udp[i];
        // Une source précise (multicast SSM, IGMPv3) : les autres sont ignorées.
        if (sip && (sip[i][0] | sip[i][1] | sip[i][2] | sip[i][3])) {
            memcpy(ops.port.mcast_sip_addr[i], sip[i], 4);
        }
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
    // Les lignes du tampon : une trame (la moitié) en entrelacé.
    out->height = f->interlaced ? f->height / 2 : f->height;
    out->second_field = f->second_field;
    out->complete = f->status == ST_FRAME_STATUS_COMPLETE || f->status == ST_FRAME_STATUS_RECONSTRUCTED;
    out->timestamp = f->timestamp;
    return 1;
}

void smtl_rx_put(void *rx, void *frame) {
    p_st20p_rx_put_frame((st20p_rx_handle)rx, (struct st_frame *)frame);
}

// Reçus, perdus (avant 2022-7), perdus pour de bon (après 2022-7), puis par patte : reçus, perdus.
void smtl_rx_stats(void *rx, uint64_t *out) {
    struct st20_rx_user_stats st;
    memset(&st, 0, sizeof(st));
    if (p_st20p_rx_get_session_stats((st20p_rx_handle)rx, &st) == 0) {
        out[0] = st.common.stat_pkts_received;
        out[1] = st.common.stat_lost_packets;
        out[2] = st.common.stat_pkts_unrecovered;
        for (int i = 0; i < 2 && i < MTL_SESSION_PORT_MAX; i++) {
            out[3 + 2 * i] = st.common.port[i].packets;
            out[4 + 2 * i] = st.common.port[i].lost_packets;
        }
    }
}

void smtl_rx_close(void *rx) {
    p_st20p_rx_free((st20p_rx_handle)rx);
}

// Envoie un flux vidéo : `n` pattes, vers `ip` / `udp`.
// `pacing` : 0 N (étroit, par défaut), 1 W (large), 2 NL (étroit linéaire) — ST 2110-21.
// `ssrc` (0 : au hasard) ; `user_time` : l'appelant donne l'instant TAI de chaque image (IPMX :
// ses rapports RTCP partent avant elle, avec le même horodatage).
void *smtl_tx_open(void *h, const char *name, int n, const int *port_index, const uint8_t (*ip)[4],
                   const uint16_t *udp, uint8_t pt, smtl_video v, int pacing, uint32_t ssrc, int user_time, char *err,
                   int errlen) {
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
    ops.port.ssrc = ssrc;
    ops.width = v.width;
    ops.height = v.height;
    ops.fps = (enum st_fps)v.fps;
    ops.interlaced = v.interlaced != 0;
    ops.input_fmt = ST_FRAME_FMT_V210;
    ops.transport_fmt = ST20_FMT_YUV_422_10BIT;
    ops.transport_pacing = pacing == 1 ? ST21_PACING_WIDE : pacing == 2 ? ST21_PACING_LINEAR : ST21_PACING_NARROW;
    ops.transport_packing = ST20_PACKING_BPM;
    ops.device = ST_PLUGIN_DEVICE_AUTO;
    ops.framebuff_cnt = 3;
    ops.flags = ST20P_TX_FLAG_BLOCK_GET;
    if (user_time) {
        ops.flags |= ST20P_TX_FLAG_USER_TIMESTAMP | ST20P_TX_FLAG_USER_PACING;
    }
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
    // Les lignes du tampon : une trame (la moitié) en entrelacé.
    out->height = f->interlaced ? f->height / 2 : f->height;
    out->second_field = f->second_field;
    out->complete = 1;
    out->timestamp = 0;
    return 1;
}

// Envoie l'image remplie ; en entrelacé, `second_field` dit quelle trame.
// `tai_ns` : l'instant d'envoi (TAI), ou 0 (le rythme de MTL).
int smtl_tx_put(void *tx, void *frame, int second_field, uint64_t tai_ns) {
    struct st_frame *f = (struct st_frame *)frame;
    f->second_field = second_field != 0;
    if (tai_ns) {
        f->tfmt = ST10_TIMESTAMP_FMT_TAI;
        f->timestamp = tai_ns;
    }
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
                    const uint16_t *udp, const uint8_t (*sip)[4], uint8_t pt, smtl_audio a, char *err, int errlen) {
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
        // Une source précise (multicast SSM, IGMPv3) : les autres sont ignorées.
        if (sip && (sip[i][0] | sip[i][1] | sip[i][2] | sip[i][3])) {
            memcpy(ops.port.mcast_sip_addr[i], sip[i], 4);
        }
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

// Reçus, perdus (avant 2022-7), perdus pour de bon (après 2022-7), puis par patte : reçus, perdus.
void smtl_arx_stats(void *rx, uint64_t *out) {
    struct st30_rx_user_stats st;
    memset(&st, 0, sizeof(st));
    if (p_st30p_rx_get_session_stats((st30p_rx_handle)rx, &st) == 0) {
        out[0] = st.common.stat_pkts_received;
        out[1] = st.common.stat_lost_packets;
        out[2] = st.common.stat_pkts_unrecovered;
        for (int i = 0; i < 2 && i < MTL_SESSION_PORT_MAX; i++) {
            out[3 + 2 * i] = st.common.port[i].packets;
            out[4 + 2 * i] = st.common.port[i].lost_packets;
        }
    }
}

void smtl_arx_close(void *rx) {
    p_st30p_rx_free((st30p_rx_handle)rx);
}

// Envoie un flux audio : `n` pattes, vers `ip` / `udp`.
void *smtl_atx_open(void *h, const char *name, int n, const int *port_index, const uint8_t (*ip)[4],
                    const uint16_t *udp, const uint8_t (*sip)[4], uint8_t pt, smtl_audio a, uint32_t ssrc, int user_time,
                    char *err, int errlen) {
    (void)sip;
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
    ops.port.ssrc = ssrc;
    ops.flags = ST30P_TX_FLAG_BLOCK_GET;
    if (user_time) {
        ops.flags |= ST30P_TX_FLAG_USER_PACING;
    }
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

int smtl_atx_put(void *tx, void *frame, uint64_t tai_ns) {
    struct st30_frame *f = (struct st30_frame *)frame;
    if (tai_ns) {
        f->tfmt = ST10_TIMESTAMP_FMT_TAI;
        f->timestamp = tai_ns;
    }
    return p_st30p_tx_put_frame((st30p_tx_handle)tx, f);
}

void smtl_atx_close(void *tx) {
    p_st30p_tx_free((st30p_tx_handle)tx);
}

// ---------- Données annexes : ST 2110-40 ----------

// Un paquet ANC (mots sur 8 bits : MTL calcule parité et somme de contrôle).
typedef struct {
    uint16_t c, line, offset, s, stream, did, sdid, size;
    uint8_t udw[255];
} smtl_anc;

#define UDW_MAX (ST40_MAX_META * 255)

static int data_open_check(char *err, int errlen) {
    if (!has_st40p) {
        set_err(err, errlen, "this MTL has no ST 2110-40 (st40p)");
        return -1;
    }
    return 0;
}

// Reçoit un flux de données (le format, entrelacé ou non, se lit dans les paquets).
void *smtl_drx_open(void *h, const char *name, int n, const int *port_index, const uint8_t (*ip)[4],
                    const uint16_t *udp, const uint8_t (*sip)[4], uint8_t pt, int interlaced, char *err, int errlen) {
    smtl *s = (smtl *)h;
    if (data_open_check(err, errlen)) {
        return NULL;
    }
    struct st40p_rx_ops ops;
    memset(&ops, 0, sizeof(ops));
    ops.name = name;
    ops.port.num_port = (uint8_t)n;
    for (int i = 0; i < n; i++) {
        snprintf(ops.port.port[i], sizeof(ops.port.port[i]), "%s", s->ports[port_index[i]]);
        memcpy(ops.port.ip_addr[i], ip[i], 4);
        ops.port.udp_port[i] = udp[i];
        // Une source précise (multicast SSM, IGMPv3) : les autres sont ignorées.
        if (sip && (sip[i][0] | sip[i][1] | sip[i][2] | sip[i][3])) {
            memcpy(ops.port.mcast_sip_addr[i], sip[i], 4);
        }
    }
    ops.port.payload_type = pt;
    ops.interlaced = interlaced != 0;
    ops.framebuff_cnt = 4;
    ops.max_udw_buff_size = UDW_MAX;
    ops.flags = ST40P_RX_FLAG_BLOCK_GET;
    st40p_rx_handle rx = p_st40p_rx_create(s->mt, &ops);
    if (!rx) {
        set_err(err, errlen, "cannot receive this ST 2110-40 flow (address?)");
        return NULL;
    }
    p_st40p_rx_set_block_timeout(rx, 200ull * 1000000ull);
    return rx;
}

// Les paquets du prochain bloc reçu (attend 200 ms au plus), copiés dans `out`
// (`max` au plus) : leur nombre, ou -1 sans bloc. `field` : 0 progressif, 2 ou 3.
int smtl_drx_get(void *rx, smtl_anc *out, int max, int *field) {
    struct st40_frame_info *f = p_st40p_rx_get_frame((st40p_rx_handle)rx);
    if (!f) {
        return -1;
    }
    int n = 0;
    for (uint32_t i = 0; i < f->meta_num && n < max; i++) {
        struct st40_meta *m = &f->meta[i];
        smtl_anc *a = &out[n];
        a->c = m->c;
        a->line = m->line_number;
        a->offset = m->hori_offset;
        a->s = m->s;
        a->stream = m->stream_num;
        a->did = m->did & 0xFF;
        a->sdid = m->sdid & 0xFF;
        uint32_t size = m->udw_size > 255 ? 255 : m->udw_size;
        if ((uint64_t)m->udw_offset + size > f->udw_buffer_fill) {
            continue;
        }
        a->size = (uint16_t)size;
        memcpy(a->udw, f->udw_buff_addr + m->udw_offset, size);
        n++;
    }
    *field = f->interlaced ? (f->second_field ? 3 : 2) : 0;
    p_st40p_rx_put_frame((st40p_rx_handle)rx, f);
    return n;
}

// Reçus, perdus (avant 2022-7), perdus pour de bon (après 2022-7), puis par patte : reçus, perdus.
void smtl_drx_stats(void *rx, uint64_t *out) {
    struct st40_rx_user_stats st;
    memset(&st, 0, sizeof(st));
    if (p_st40p_rx_get_session_stats((st40p_rx_handle)rx, &st) == 0) {
        out[0] = st.common.stat_pkts_received;
        out[1] = st.common.stat_lost_packets;
        out[2] = st.common.stat_pkts_unrecovered;
        for (int i = 0; i < 2 && i < MTL_SESSION_PORT_MAX; i++) {
            out[3 + 2 * i] = st.common.port[i].packets;
            out[4 + 2 * i] = st.common.port[i].lost_packets;
        }
    }
}

void smtl_drx_close(void *rx) {
    p_st40p_rx_free((st40p_rx_handle)rx);
}

// Envoie un flux de données, `fps` (enum st_fps) celle des trames en entrelacé :
// un bloc par période, MTL donne le rythme.
void *smtl_dtx_open(void *h, const char *name, int n, const int *port_index, const uint8_t (*ip)[4],
                    const uint16_t *udp, uint8_t pt, int fps, int interlaced, char *err, int errlen) {
    smtl *s = (smtl *)h;
    if (data_open_check(err, errlen)) {
        return NULL;
    }
    struct st40p_tx_ops ops;
    memset(&ops, 0, sizeof(ops));
    ops.name = name;
    ops.port.num_port = (uint8_t)n;
    for (int i = 0; i < n; i++) {
        snprintf(ops.port.port[i], sizeof(ops.port.port[i]), "%s", s->ports[port_index[i]]);
        memcpy(ops.port.dip_addr[i], ip[i], 4);
        ops.port.udp_port[i] = udp[i];
    }
    ops.port.payload_type = pt;
    ops.fps = (enum st_fps)fps;
    ops.interlaced = interlaced != 0;
    ops.framebuff_cnt = 3;
    ops.max_udw_buff_size = UDW_MAX;
    ops.flags = ST40P_TX_FLAG_BLOCK_GET;
    st40p_tx_handle tx = p_st40p_tx_create(s->mt, &ops);
    if (!tx) {
        set_err(err, errlen, "cannot send this ST 2110-40 flow (address?)");
        return NULL;
    }
    p_st40p_tx_set_block_timeout(tx, 200ull * 1000000ull);
    return tx;
}

// Attend un bloc libre (200 ms au plus) et l'envoie avec ces `n` paquets
// (ST40_MAX_META au plus) : 0, ou -1.
int smtl_dtx_send(void *tx, const smtl_anc *in, int n) {
    struct st40_frame_info *f = p_st40p_tx_get_frame((st40p_tx_handle)tx);
    if (!f) {
        return -1;
    }
    uint32_t fill = 0;
    uint32_t k = 0;
    for (int i = 0; i < n && k < ST40_MAX_META; i++) {
        const smtl_anc *a = &in[i];
        if (fill + a->size > f->udw_buffer_size) {
            break;
        }
        struct st40_meta *m = &f->meta[k++];
        m->c = a->c;
        m->line_number = a->line;
        m->hori_offset = a->offset;
        m->s = a->s;
        m->stream_num = a->stream;
        m->did = a->did;
        m->sdid = a->sdid;
        m->udw_size = a->size;
        m->udw_offset = (uint16_t)fill;
        memcpy(f->udw_buff_addr + fill, a->udw, a->size);
        fill += a->size;
    }
    f->meta_num = k;
    f->udw_buffer_fill = fill;
    return p_st40p_tx_put_frame((st40p_tx_handle)tx, f);
}

void smtl_dtx_close(void *tx) {
    p_st40p_tx_free((st40p_tx_handle)tx);
}

// ---------- La vidéo compressée : ST 2110-22 (JPEG XS) ----------

// Une image en trois plans (Y, Cb, Cr : 4:2:2, 10 bits dans 16, petit-boutiste), celle que le
// greffon JPEG XS prend et rend.
typedef struct {
    void *frame;
    uint8_t *data[3];
    uint64_t linesize[3];
    uint32_t width, height;
    int second_field;
    int complete;
    uint64_t timestamp;
} smtl_frame3;

int smtl_has_st22p(void) {
    return has_st22p;
}

static void frame3_of(struct st_frame *f, smtl_frame3 *out) {
    out->frame = f;
    for (int i = 0; i < 3; i++) {
        out->data[i] = (uint8_t *)f->addr[i];
        out->linesize[i] = f->linesize[i];
    }
    out->width = f->width;
    out->height = f->interlaced ? f->height / 2 : f->height;
    out->second_field = f->second_field;
    out->timestamp = f->timestamp;
}

// Reçoit un flux JPEG XS : décodé par le greffon en 4:2:2 10 bits planaire.
void *smtl22_rx_open(void *h, const char *name, int n, const int *port_index, const uint8_t (*ip)[4],
                     const uint16_t *udp, const uint8_t (*sip)[4], uint8_t pt, smtl_video v, char *err, int errlen) {
    if (!has_st22p) {
        set_err(err, errlen, "this MTL has no ST 2110-22");
        return NULL;
    }
    smtl *s = (smtl *)h;
    struct st22p_rx_ops ops;
    memset(&ops, 0, sizeof(ops));
    ops.name = name;
    ops.port.num_port = (uint8_t)n;
    for (int i = 0; i < n; i++) {
        snprintf(ops.port.port[i], sizeof(ops.port.port[i]), "%s", s->ports[port_index[i]]);
        memcpy(ops.port.ip_addr[i], ip[i], 4);
        ops.port.udp_port[i] = udp[i];
        if (sip && (sip[i][0] | sip[i][1] | sip[i][2] | sip[i][3])) {
            memcpy(ops.port.mcast_sip_addr[i], sip[i], 4);
        }
    }
    ops.port.payload_type = pt;
    ops.width = v.width;
    ops.height = v.height;
    ops.fps = (enum st_fps)v.fps;
    ops.interlaced = v.interlaced != 0;
    ops.output_fmt = ST_FRAME_FMT_YUV422PLANAR10LE;
    ops.pack_type = ST22_PACK_CODESTREAM;
    ops.codec = ST22_CODEC_JPEGXS;
    ops.device = ST_PLUGIN_DEVICE_AUTO;
    ops.framebuff_cnt = 3;
    ops.flags = ST22P_RX_FLAG_BLOCK_GET;
    st22p_rx_handle rx = p_st22p_rx_create(s->mt, &ops);
    if (!rx) {
        set_err(err, errlen, "cannot receive this JPEG XS flow (plugin, address, format?)");
        return NULL;
    }
    p_st22p_rx_set_block_timeout(rx, 200ull * 1000000ull);
    return rx;
}

int smtl22_rx_get(void *rx, smtl_frame3 *out) {
    struct st_frame *f = p_st22p_rx_get_frame((st22p_rx_handle)rx);
    if (!f) {
        return 0;
    }
    frame3_of(f, out);
    out->complete = f->status == ST_FRAME_STATUS_COMPLETE || f->status == ST_FRAME_STATUS_RECONSTRUCTED;
    return 1;
}

void smtl22_rx_put(void *rx, void *frame) {
    p_st22p_rx_put_frame((st22p_rx_handle)rx, (struct st_frame *)frame);
}

void smtl22_rx_close(void *rx) {
    p_st22p_rx_free((st22p_rx_handle)rx);
}

// Envoie un flux JPEG XS : `codestream_size` octets par image (par trame en entrelacé), le
// débit voulu.
void *smtl22_tx_open(void *h, const char *name, int n, const int *port_index, const uint8_t (*ip)[4],
                     const uint16_t *udp, uint8_t pt, smtl_video v, uint64_t codestream_size, uint32_t ssrc, int user_time,
                     char *err, int errlen) {
    if (!has_st22p) {
        set_err(err, errlen, "this MTL has no ST 2110-22");
        return NULL;
    }
    smtl *s = (smtl *)h;
    struct st22p_tx_ops ops;
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
    ops.input_fmt = ST_FRAME_FMT_YUV422PLANAR10LE;
    ops.pack_type = ST22_PACK_CODESTREAM;
    ops.codec = ST22_CODEC_JPEGXS;
    ops.device = ST_PLUGIN_DEVICE_AUTO;
    ops.quality = ST22_QUALITY_MODE_QUALITY;
    ops.codestream_size = (size_t)codestream_size;
    ops.port.ssrc = ssrc;
    ops.framebuff_cnt = 3;
    ops.flags = ST22P_TX_FLAG_BLOCK_GET;
    if (user_time) {
        ops.flags |= ST22P_TX_FLAG_USER_TIMESTAMP | ST22P_TX_FLAG_USER_PACING;
    }
    st22p_tx_handle tx = p_st22p_tx_create(s->mt, &ops);
    if (!tx) {
        set_err(err, errlen, "cannot send this JPEG XS flow (plugin, address, format?)");
        return NULL;
    }
    p_st22p_tx_set_block_timeout(tx, 200ull * 1000000ull);
    return tx;
}

int smtl22_tx_get(void *tx, smtl_frame3 *out) {
    struct st_frame *f = p_st22p_tx_get_frame((st22p_tx_handle)tx);
    if (!f) {
        return 0;
    }
    frame3_of(f, out);
    out->complete = 1;
    return 1;
}

int smtl22_tx_put(void *tx, void *frame, int second_field, uint64_t tai_ns) {
    struct st_frame *f = (struct st_frame *)frame;
    f->second_field = second_field != 0;
    if (tai_ns) {
        f->tfmt = ST10_TIMESTAMP_FMT_TAI;
        f->timestamp = tai_ns;
    }
    return p_st22p_tx_put_frame((st22p_tx_handle)tx, f);
}

void smtl22_tx_close(void *tx) {
    p_st22p_tx_free((st22p_tx_handle)tx);
}
