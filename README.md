# 免责声明 
本工具仅面向合法授权的企业安全建设行为，如您需要测试本工具的可用性，请自行搭建靶机环境。

在使用本工具进行检测时，您应确保该行为符合当地的法律法规，并且已经取得了足够的授权。请勿对非授权目标进行扫描。

此工具仅限于安全研究和教学，用户承担因使用此工具而导致的所有法律和相关责任！ 作者不承担任何法律和相关责任！

如您在使用本工具的过程中存在任何非法行为，您需自行承担相应后果，我们将不承担任何法律及连带责任。



# 基于rustdesk修改的远程桌面软件  

原版地址 https://github.com/rustdesk/rustdesk

使用方法：

./server -i 192.168.93.217

-i 公网地址

-t ID服务器监听地址，默认0.0.0.0:21116

-u 中继服务器监听地址，默认0.0.0.0:21117


agent.exe -s 192.168.93.217 -u asdf -p 1234

-s 自定义服务器地址，默认官方服务器

-u 自定义ID

-p 自定义密码



## 修改部分

1、精简界面部分功能代码,提取agent受控端核心代码 

2、自定义服务器、账号、密码

3、将chat功能改为cmd执行命令窗口 



## 使用如图

 ![](https://github.com/YDHCUI/rustdesk/blob/main/images/0.png)

 ![](https://github.com/YDHCUI/rustdesk/blob/main/images/1.png)

 ![](https://github.com/YDHCUI/rustdesk/blob/main/images/2.png)

 ![](https://github.com/YDHCUI/rustdesk/blob/main/images/3.png)
 
 ![](https://github.com/YDHCUI/rustdesk/blob/main/images/4.png)
 
