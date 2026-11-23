/* Standalone C11 consumer. No CLI or Rust tools needed at runtime. */
#include "mynofy.h"
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>

static int checked(int32_t code, const mynofy_error *error) {
    if (code == MYNOFY_OK) return 1;
    fprintf(stderr,"mynofy status=%" PRId32 ": %s (outcome_unknown=%u)\n",code,error->message,error->outcome_unknown);
    return 0;
}
static void print_view(mynofy_str view) {if(view.len)fwrite(view.data,1,view.len,stdout);}
int main(int argc, char **argv) {
    if(argc != 5) {fprintf(stderr,"Usage: %s SOCKET KEY.pk8 OWNER TOPIC\n",argv[0]);return 2;}
    mynofy_key *key=NULL;mynofy_client *client=NULL;mynofy_response *response=NULL;
    mynofy_error error;int result=EXIT_FAILURE;
    unsigned char der[4096];FILE *file=fopen(argv[2],"rb");
    if(!file) {perror("key");return EXIT_FAILURE;}
    size_t length=fread(der,1,sizeof der,file);int complete=feof(file)&&!ferror(file);fclose(file);
    if(!complete || !checked(mynofy_key_parse(der,length,&key,&error),&error))goto done;
    /* Erase this application's temporary copy; the SDK owns its own key copy. */
    for(size_t i=0;i<sizeof der;i++)((volatile unsigned char *)der)[i]=0;
    mynofy_config config=MYNOFY_CONFIG_INIT;config.socket_path=argv[1];config.owner=argv[3];config.key=key;
    if(!checked(mynofy_connect(&config,&client,&error),&error))goto done;
    mynofy_connection_info info;
    if(!checked(mynofy_client_info(client,&info,&error),&error))goto done;
    printf("Owner: ");print_view(info.owner);printf(" restart=%" PRIu32 "\n",info.restart_counter);
    const uint8_t text[]="Hello from C";
    if(!checked(mynofy_publish_bytes(client,argv[4],text,sizeof text-1,"text/plain",NULL,&response,&error),&error))goto done;
    printf("Published: ");print_view(mynofy_response_message_id(response));putchar('\n');
    mynofy_response_free(response);response=NULL;
    if(!checked(mynofy_poll(client,argv[4],NULL,20,&response,&error),&error))goto done;
    for(size_t i=0;i<mynofy_response_message_count(response);i++) {
        mynofy_message item;
        if(!checked(mynofy_response_message_at(response,i,&item,&error),&error))goto done;
        print_view(item.id);
        if(item.payload_kind==MYNOFY_BYTES)printf(" bytes=%zu",item.data.len);
        else if(item.payload_kind==MYNOFY_EVENT)printf(" event=%" PRIu32 "/%" PRIu32 "/%" PRIu32,item.event.category,item.event.action,item.event.status);
        putchar('\n');
    }
    printf("Next cursor: ");print_view(mynofy_response_next_cursor(response));putchar('\n');
    /* Persist/advance only after your application has taken responsibility. */
    result=EXIT_SUCCESS;
done:
    mynofy_response_free(response);mynofy_client_free(client);mynofy_key_free(key);
    return result;
}
