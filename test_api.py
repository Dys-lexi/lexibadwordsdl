import json
import requests
import threading
import time
import traceback
import random
import functools
from psycopg2.extras import Json
from initsql import *
from concurrent.futures import ThreadPoolExecutor, as_completed
print("started")
def dothing(matchid):
    try:
        print("doing",matchid)
        now = time.time()
        with querywrapper() as query:
            query.execute("SELECT parsedcorrectly FROM match WHERE match_id = %s LIMIT 1",(matchid,))
            output = query.fetchone()
        if output and output[0]: 
            print("failed here - already questioned")
            return (matchid,False)
        r = requests.get(f"https://api.deadlock-api.com/v1/matches/{matchid}/salts",params={"disable_steam":"true"})
        if r.status_code == 429 or r.status_code == 500:
            return (matchid,True)
        if "application/json" not in r.headers.get("Content-Type", ""):
            print(r.text)
            print("salt failed for",matchid)
            return matchid
        print("got salt",matchid)

        if "demo_url" in r.json() and not r.json()["demo_url"]:
            print("failed here2",r.text)
            return (matchid,False)
        elif "demo_url" not in r.json():
            return (matchid,True)

        url = r.json()["demo_url"]
        
        payload = {"url": url, "run": ["deadlock-chat-messages"]}
        print("downloading / parsing demo")
        try:
            if random.randint(0,1) and False:
                # try:
                #     print("doing dis")
                #     request = requests.post("http://100.95.78.79:80/parse", 
                #                     json=payload,
                #                     headers={"Content-Type": "application/json"})
                # except:
                #     print("doing dat")
                    request = requests.post("http://haste:80/parse", 
                                    json=payload,
                                    headers={"Content-Type": "application/json"})
        
            else:
                request = requests.post("http://haste:80/parse", 
                                json=payload,  timeout=300,
                                headers={"Content-Type": "application/json"})
        except:
            traceback.print_exc()
            return (matchid,True)
        
        print("Parsed demo",matchid,"in",f"{time.time()-now:.3f} seconds")
        
        # print(json.dumps(request.json(),indent=4))
        # with open ("xyz.json","w") as f:
        #     (json.dump(request.json(),f,indent=4))
        x = request.json()
        if "results" not in x:
            print("failed3",x)
            return (matchid,True)
    except Exception:
        traceback.print_exc()

    return request.json()


def dothingparttwo(matchid):
    with open("xyz.json","r") as f:
        return json.load(f)

def getnewgames():

    with querywrapper() as query:
        query.execute("SELECT match_id FROM match WHERE parsedcorrectly = false ORDER BY match_id LIMIT 1000")
        unfinisheddownloads = query.fetchall()
        if not unfinisheddownloads:
            query.execute("SELECT match_id FROM match WHERE parsedcorrectly = true ORDER BY match_id DESC LIMIT 1")
            output = query.fetchone()
            mostrecentmatchid = output and output[0] or 138067450 or 138067446
            print(output)
            print(mostrecentmatchid,"mostrecent")
    if len(unfinisheddownloads) < 10:
        print("doing new matches")
        r = requests.get( 'https://api.deadlock-api.com/v1/matches/metadata',params={"include_info":"true","limit":10000,"format":"json","order_direction":"asc","min_match_id":mostrecentmatchid,"max_unix_timestamp":int(time.time()-3600*5)})
    else:
        print("redoing broken matches")
        r = requests.get( 'https://api.deadlock-api.com/v1/matches/metadata',params={"include_info":"true","limit":10000,"format":"json","order_direction":"asc","match_ids":",".join(list(map(lambda x: str(x[0]),unfinisheddownloads)))})
    print("pulled matches")
    if "application/json" not in r.headers.get("Content-Type", ""):
        print(r.text)
        print("failed to parse match metadata")
        time.sleep(60)
    yield from list(map(lambda x: x["match_id"],r.json()))



def main():
    while True:
        with ThreadPoolExecutor(max_workers=12) as executor:
            games = [
                executor.submit(dothing, i)
                for i in getnewgames()
            ]

            for i, game in enumerate(as_completed(games)):
                try:
                    parsegame(game.result(),i)
                except Exception:
                    traceback.print_exc()            
                
        print("sleep time!")
        time.sleep(5)

# def my_function(i):
#     print(f"Task {i} started")
#     time.sleep(10)  # Simulate blocking for 10 seconds
#     print(f"Task {i} finished")

# with ThreadPoolExecutor(max_workers=5) as executor:
#     executor.map(my_function, range(100))

def parsegame(game,i):
    # print(type(game))
    if type(game) == tuple:
        print("could not parse",game)
        if game[1]:
            with querywrapper() as query:
                query.execute("INSERT INTO match (match_id,parsedcorrectly) VALUES (%s,%s) ON CONFLICT DO NOTHING",(game[0],False))
                query.commit()
        return
    game = game["results"]["deadlock-chat-messages"]
    print("saving game",game["match"]["match_id"])
    with querywrapper() as query:
        query.execute("SELECT table_name,column_name FROM information_schema.columns")
        rows = functools.reduce(lambda a,b: {**a,b[0]:[*a.get(b[0],[]),b[1]]}, query.fetchall(),{})
    # a is the accumulater b is the iterable
    with querywrapper() as query:
        query.execute("DELETE FROM match WHERE match_id = %s",(game["match"]["match_id"],))
        query.execute("INSERT INTO match (match_id,parsedcorrectly) VALUES (%s,%s)",(game["match"]["match_id"],False))
        query.commit()
        # print(f"UPDATE match SET {",".join(list(map(lambda x: f"{x} = %s" ,list(filter(lambda x: x in rows["match"],game["match"])))))} WHERE match_id = %s")
        # print((*list(map(lambda x: x[1],filter(lambda x: x[0] in rows["match"],game["match"].items()))),game["match"]["match_id"]))
        game["match"] = {**game["match"],"parsedcorrectly":True,"account_ids": list(map(lambda x: x["account_id"],game["match"]["players"]))}
        # print(f"UPDATE match SET {",".join(list(map(lambda x: f"{x} = %s" ,list(filter(lambda x: x in rows["match"],game["match"])))))} WHERE match_id = %s")
        query.execute(f"UPDATE match SET {",".join(list(map(lambda x: f"{x} = %s" ,list(filter(lambda x: x in rows["match"],game["match"])))))} WHERE match_id = %s",(*list(map(lambda x: type(x[1]) == list and x[0] != "account_ids" and Json(x[1]) or x[1],filter(lambda x: x[0] in rows["match"],game["match"].items()))),game["match"]["match_id"]))
        for chat_number, data in enumerate(game["chat"]):
            data = {**game["match"],**data,"chat_number":chat_number}
            # print(data)
            # print(rows[table])
            # print(list(filter(lambda x: x in rows[table],data)))
            # print(f"INSERT INTO {table} ({",".join(list(filter(lambda x: x in rows[table],data)))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows[table],data))))})")
            # print(list(map(lambda x: x[1],filter(lambda x: x[0] in rows[table],data.items()))))
            query.execute(f"INSERT INTO {"chat"} ({",".join(list(filter(lambda x: x in rows["chat"],data)))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows["chat"],data))))})",list(map(lambda x: x[1],filter(lambda x: x[0] in rows["chat"],data.items()))))
        for event_number,data in enumerate(game["events"]):
            
            # print(list(map(lambda x: x.removeprefix("attacker_") ,rows["events"])),"wqdwqdwq")
            # print("eee",data["assists"])
            for i,thing in enumerate(data["assists"] or []) :
                data["assists"][i] = dict(filter(lambda x: x[0] in list(map(lambda x: x.removeprefix("killer_") ,rows["events"])),thing.items()))
            data = {**game["match"],**data,"event_number":event_number,"assists":Json(data["assists"])}
            # print(json.dumps(data,indent=4))
            # break
            # print(f"INSERT INTO {"events"} ({",".join(list(filter(lambda x: x in rows["events"],data)))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows["events"],data))))})")
            # print(list(map(lambda x: x[1],filter(lambda x: x[0] in rows["events"],data.items()))))
            query.execute(f"INSERT INTO {"events"} ({",".join(list(filter(lambda x: x in rows["events"],data)))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows["events"],data))))})",list(map(lambda x: x[1],filter(lambda x: x[0] in rows["events"],data.items()))))
        for data in game["match"]["players"]:
            # print(f"INSERT INTO {"players"} ({",".join(list(filter(lambda x: x in rows["players"],data)))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows["players"],data))))}) ON CONFLICT (account_id) DO UPDATE SET {",".join(list(map(lambda x: f"{x} = %s" ,list(filter(lambda x: x in rows["players"],data)))))}")
            # print(f"INSERT INTO {"players"} ({",".join(list(filter(lambda x: x in rows["players"],{**data,"match_ids":[game["match"]["match_id"]]})))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows["players"],data))))}) ON CONFLICT (account_id) DO UPDATE SET {",".join(list(map(lambda x: f"{x} = %s",list(filter(lambda x: x in rows["players"],data)))))}, match_ids = players.match_ids || %s")
            # print((*list(map(lambda x: x[1],filter(lambda x: x[0] in rows["players"],{**data,"match_ids":[game["match"]["match_id"]]}.items()))),*list(map(lambda x: x[1],filter(lambda x: x[0] in rows["players"],{**data,"match_ids":[game["match"]["match_id"]]}.items())))))
            query.execute(f"INSERT INTO {"players"} ({",".join(list(filter(lambda x: x in rows["players"],{**data,"match_ids":[game["match"]["match_id"]]})))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows["players"],{**data,"match_ids":[game["match"]["match_id"]]}))))}) ON CONFLICT (account_id) DO UPDATE SET {",".join(list(map(lambda x: f"{x} = %s",list(filter(lambda x: x in rows["players"],data)))))}, match_ids = players.match_ids || %s",(*list(map(lambda x: x[1],filter(lambda x: x[0] in rows["players"],{**data,"match_ids":[game["match"]["match_id"]]}.items()))),*list(map(lambda x: x[1],filter(lambda x: x[0] in rows["players"],{**data,"match_ids":[game["match"]["match_id"]]}.items())))))
        query.commit()
    print("saved game",game["match"]["match_id"])
        
        

    


main()