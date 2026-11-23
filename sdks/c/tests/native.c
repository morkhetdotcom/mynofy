#define _POSIX_C_SOURCE 200809L
#include "mynofy.h"
#include <assert.h>
#include <fcntl.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>
#include <time.h>
#ifdef __linux__
#include <dirent.h>
static size_t io_threads(void) {
    DIR *directory=opendir("/proc/self/task");assert(directory);struct dirent *entry;size_t count=0;
    while((entry=readdir(directory))) {
        if(entry->d_name[0]=='.')continue;
        char path[512],name[64];int n=snprintf(path,sizeof path,"/proc/self/task/%s/comm",entry->d_name);assert(n>0 && (size_t)n<sizeof path);
        FILE *file=fopen(path,"r");if(!file)continue;
        if(fgets(name,sizeof name,file) && strcmp(name,"mynofy-c-io\n")==0)count++;
        assert(fclose(file)==0);
    }
    assert(closedir(directory)==0);return count;
}
#endif

static mynofy_error error;
static void ok(int32_t status) {
    if (status != MYNOFY_OK) {fprintf(stderr,"status=%" PRId32 " unknown=%u: %s (%s)\n",status,error.outcome_unknown,error.message,error.broker_code);abort();}
}
static int equal(mynofy_str view,const char *text) {return view.len==strlen(text) && (view.len==0 || memcmp(view.data,text,view.len)==0);}
static void copy(mynofy_str view,char *buffer,size_t capacity) {assert(view.len<capacity);if(view.len)memcpy(buffer,view.data,view.len);buffer[view.len]=0;}
static mynofy_key *load(const char *path) {
    unsigned char der[4096];FILE *file=fopen(path,"rb");assert(file);
    size_t size=fread(der,1,sizeof der,file);assert(!ferror(file));assert(feof(file));assert(fclose(file)==0);
    mynofy_key *key=NULL;ok(mynofy_key_parse(der,size,&key,&error));return key;
}
static void release(mynofy_response **response) {mynofy_response_free(*response);*response=NULL;}
static mynofy_config config(int argc,char **argv,mynofy_key *key) {
    assert(argc>=7);mynofy_config c=MYNOFY_CONFIG_INIT;c.owner="local";c.key=key;
    if(strcmp(argv[2],"unix")==0)c.socket_path=argv[3];
    else {assert(strcmp(argv[2],"wss")==0);c.url=argv[3];c.ca_file=argv[4];}
    return c;
}
static void roundtrip(mynofy_client *client) {
    const char *topic="local/events";mynofy_response *response=NULL;
    mynofy_connection_info info;ok(mynofy_client_info(client,&info,&error));assert(equal(info.owner,"local"));assert(info.client_id.len>32);
    ok(mynofy_get_catalog(client,"mynofy",1,&response,&error));assert(mynofy_response_type(response)==MYNOFY_CATALOG);
    mynofy_bytes catalog=mynofy_response_catalog(response);assert(catalog.len>100);release(&response);
    ok(mynofy_get_permissions(client,topic,&response,&error));assert(mynofy_response_type(response)==MYNOFY_PERMISSIONS);
    assert(equal(mynofy_response_owner(response),"local"));assert(mynofy_response_acl_revision(response).len>0);
    assert(mynofy_response_permission_count(response)==3);
    int publish=0;for(size_t i=0;i<mynofy_response_permission_count(response);i++)publish|=equal(mynofy_response_permission_at(response,i),"publish");assert(publish);release(&response);
    unsigned char bytes[4096];for(size_t i=0;i<sizeof bytes;i++)bytes[i]=(unsigned char)i;
    ok(mynofy_publish_bytes(client,topic,bytes,sizeof bytes,"application/octet-stream",NULL,&response,&error));
    assert(mynofy_response_type(response)==MYNOFY_RESULT);assert(mynofy_response_expires(response).len>0);
    char first[32],second[32],cursor[32];copy(mynofy_response_message_id(response),first,sizeof first);release(&response);
    mynofy_event event={0};event.catalog=1;event.catalog_version=1;event.category=12;event.action=42;event.status=2;event.parameter_count=3;
    event.parameters[0].type=MYNOFY_U64;event.parameters[0].value.u64_value=UINT64_MAX;
    event.parameters[1].type=MYNOFY_I64;event.parameters[1].value.i64_value=INT64_MIN;
    event.parameters[2].type=MYNOFY_BOOL;event.parameters[2].value.bool_value=0;
    ok(mynofy_publish_event(client,topic,&event,NULL,&response,&error));copy(mynofy_response_message_id(response),second,sizeof second);release(&response);
    ok(mynofy_poll(client,topic,NULL,1,&response,&error));assert(mynofy_response_message_count(response)==1);assert(mynofy_response_has_more(response));
    mynofy_message item;ok(mynofy_response_message_at(response,0,&item,&error));assert(item.payload_kind==MYNOFY_BYTES);
    mynofy_str producer=mynofy_response_message_producer_id(response,0);
    assert(producer.len==info.client_id.len && memcmp(producer.data,info.client_id.data,producer.len)==0);
    assert(mynofy_response_message_producer_id(response,1).len==0);
    assert(mynofy_response_message_producer_id(NULL,0).len==0);
    assert(item.data.len==sizeof bytes && memcmp(item.data.data,bytes,sizeof bytes)==0);assert(equal(item.content_type,"application/octet-stream"));
    copy(mynofy_response_next_cursor(response),cursor,sizeof cursor);
    assert(mynofy_response_message_at(response,1,&item,&error)==MYNOFY_INVALID);assert(item.id.len==0);
    release(&response);
    ok(mynofy_poll(client,topic,cursor,20,&response,&error));assert(mynofy_response_message_count(response)==1);
    ok(mynofy_response_message_at(response,0,&item,&error));assert(item.payload_kind==MYNOFY_EVENT);assert(item.event.parameter_count==3);
    assert(item.event.parameters[0].type==MYNOFY_U64 && item.event.parameters[0].value.u64_value==UINT64_MAX);
    assert(item.event.parameters[1].type==MYNOFY_I64 && item.event.parameters[1].value.i64_value==INT64_MIN);
    assert(item.event.parameters[2].type==MYNOFY_BOOL && item.event.parameters[2].value.bool_value==0);
    copy(mynofy_response_next_cursor(response),cursor,sizeof cursor);release(&response);
    ok(mynofy_poll(client,topic,cursor,20,&response,&error));assert(mynofy_response_message_count(response)==0);release(&response);
    ok(mynofy_get_message(client,first,&response,&error));ok(mynofy_response_message_at(response,0,&item,&error));release(&response);
    ok(mynofy_get_message(client,second,&response,&error));ok(mynofy_response_message_at(response,0,&item,&error));release(&response);
    ok(mynofy_list_messages(client,topic,NULL,1,&response,&error));assert(mynofy_response_message_count(response)==1);
    char own_cursor[4097];copy(mynofy_response_next_cursor(response),own_cursor,sizeof own_cursor);assert(own_cursor[0]);
    ok(mynofy_response_message_at(response,0,&item,&error));assert(item.payload_kind==MYNOFY_NO_PAYLOAD && item.created_at.len>0);release(&response);
    ok(mynofy_list_messages(client,topic,own_cursor,1,&response,&error));assert(mynofy_response_message_count(response)==1);release(&response);
    /* Fail locally, keep the connection usable, never dereference invalid buffers. */
    assert(mynofy_publish_bytes(client,topic,NULL,1,NULL,NULL,&response,&error)==MYNOFY_INVALID && response==NULL);
    assert(mynofy_publish_bytes(client,topic,bytes,SIZE_MAX,NULL,NULL,&response,&error)==MYNOFY_INVALID);
    assert(mynofy_poll(client,topic,"bad",20,&response,&error)==MYNOFY_INVALID);
    assert(mynofy_poll(client,topic,NULL,0,&response,&error)==MYNOFY_INVALID);
    event.parameters[2].value.bool_value=2;assert(mynofy_publish_event(client,topic,&event,NULL,&response,&error)==MYNOFY_INVALID);
    event.parameter_count=4;assert(mynofy_publish_event(client,topic,&event,NULL,&response,&error)==MYNOFY_INVALID);
    assert(mynofy_publish_bytes(client,"\xff",bytes,1,NULL,NULL,&response,&error)==MYNOFY_INVALID);
    assert(mynofy_get_message(client,first,NULL,&error)==MYNOFY_INVALID);
    /* Empty binary payload is distinct from missing payload. */
    ok(mynofy_publish_bytes(client,topic,NULL,0,NULL,NULL,&response,&error));release(&response);
    /* A response owns its buffers independently of the client/session lifetime. */
    ok(mynofy_get_message(client,first,&response,&error));ok(mynofy_response_message_at(response,0,&item,&error));
    mynofy_client_close(client);assert(memcmp(item.data.data,bytes,sizeof bytes)==0);
    mynofy_response *closed=NULL;assert(mynofy_get_catalog(client,"mynofy",1,&closed,&error)==MYNOFY_CLOSED);assert(closed==NULL);
    release(&response);
}
static void denied(mynofy_client *client) {
    mynofy_response *response=NULL;
    assert(mynofy_publish_bytes(client,"local/events",NULL,0,NULL,NULL,&response,&error)==MYNOFY_BROKER);
    assert(response==NULL && !error.outcome_unknown && strcmp(error.broker_code,"forbidden")==0);
    ok(mynofy_get_catalog(client,"mynofy",1,&response,&error));release(&response);
}
static void rotation(mynofy_client *client,mynofy_config *c,const char *file) {
    mynofy_response *response=NULL;const uint8_t data[]={0,1,255};
    ok(mynofy_publish_bytes(client,"local/events",data,sizeof data,NULL,NULL,&response,&error));
    char message[32],stable[256];copy(mynofy_response_message_id(response),message,sizeof message);release(&response);
    mynofy_connection_info info;ok(mynofy_client_info(client,&info,&error));copy(info.client_id,stable,sizeof stable);
    mynofy_key *fresh=NULL;ok(mynofy_key_generate(&fresh,&error));mynofy_bytes der=mynofy_key_pkcs8(fresh);
    int fd=open(file,O_WRONLY|O_CREAT|O_EXCL,0600);assert(fd>=0);assert(write(fd,der.data,der.len)==(ssize_t)der.len);assert(fsync(fd)==0);assert(close(fd)==0);
    ok(mynofy_rotate(client,fresh,&response,&error));mynofy_rotation_info rotated;ok(mynofy_response_rotation(response,&rotated,&error));
    assert(equal(rotated.client_id,stable));mynofy_str token=mynofy_key_token_id(fresh);assert(rotated.token_id.len==token.len && memcmp(rotated.token_id.data,token.data,token.len)==0);assert(rotated.valid_until.len>0);release(&response);
    assert(mynofy_get_catalog(client,"mynofy",1,&response,&error)==MYNOFY_CLOSED);
    mynofy_client *old=NULL;assert(mynofy_connect(c,&old,&error)!=MYNOFY_OK && old==NULL);
    c->key=fresh;c->client_id=stable;mynofy_client *next=NULL;ok(mynofy_connect(c,&next,&error));
    /* Connect retains its own credential copy. */
    mynofy_key_free(fresh);fresh=NULL;
    ok(mynofy_get_message(next,message,&response,&error));mynofy_message item;ok(mynofy_response_message_at(response,0,&item,&error));
    assert(equal(mynofy_response_message_producer_id(response,0),stable));
    mynofy_client_free(next);assert(item.data.len==sizeof data && memcmp(item.data.data,data,sizeof data)==0);release(&response);
}
int main(int argc,char **argv) {
    mynofy_key *key=load(argc>5?argv[5]:"");mynofy_config c=config(argc,argv,key);mynofy_client *client=NULL;
    /* Size/version rejection happens before pointer-bearing config fields are consumed. */
    mynofy_config wrong=c;wrong.struct_size=1;assert(mynofy_connect(&wrong,&client,&error)==MYNOFY_INVALID && client==NULL);
    wrong=c;wrong.abi_version=1;assert(mynofy_connect(&wrong,&client,&error)==MYNOFY_INVALID);
    assert(mynofy_connect(NULL,&client,&error)==MYNOFY_INVALID);
    if(strcmp(argv[1],"tls-reject")==0) {
        assert(mynofy_connect(&c,&client,&error)==MYNOFY_TRANSPORT && client==NULL);
        mynofy_key_free(key);puts("TLS rejected");return 0;
    }
    ok(mynofy_connect(&c,&client,&error));
    if(strcmp(argv[1],"denied")==0)denied(client);
    else if(strcmp(argv[1],"rotate")==0)rotation(client,&c,argv[6]);
    else {assert(strcmp(argv[1],"roundtrip")==0);roundtrip(client);}
    mynofy_client_free(client);mynofy_key_free(key);
#ifdef __linux__
    /* The OS task table can briefly retain an exiting thread after shutdown. */
    struct timespec delay={0,1000000};
    for(unsigned i=0;i<1000 && io_threads()!=0;i++)nanosleep(&delay,NULL);
    assert(io_threads()==0);
#endif
    mynofy_key_free(NULL);mynofy_client_free(NULL);mynofy_response_free(NULL);
    puts("native C checks passed");return 0;
}
